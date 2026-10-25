package main

import (
	"context"
	"crypto/ecdsa"
	"crypto/tls"
	"crypto/x509"
	"encoding/json"
	"encoding/pem"
	"net"
	"net/http"
	"net/http/httptest"
	"net/url"
	"strings"
	"sync"
	"testing"
	"time"

	"tailscale.com/client/local"
	"tailscale.com/ipn/ipnstate"
)

// localAPIRecorder is a LocalAPI behind the real tailscale.com/client/local
// client. It records every request and issues the node's certificate only for
// a request shaped like the one the sidecar means to make. WhoIs is answered
// as the owner and left out of the record.
type localAPIRecorder struct {
	mu   sync.Mutex
	seen []string
}

func (r *localAPIRecorder) requests() []string {
	r.mu.Lock()
	defer r.mu.Unlock()
	return append([]string(nil), r.seen...)
}

func newLocalAPIRecorder(t *testing.T) (*local.Client, *localAPIRecorder) {
	t.Helper()
	cert, _ := selfSigned(t, testDNSName, time.Now().Add(time.Hour))
	keyDER, err := x509.MarshalPKCS8PrivateKey(cert.PrivateKey.(*ecdsa.PrivateKey))
	if err != nil {
		t.Fatal(err)
	}
	pair := append(pem.EncodeToMemory(&pem.Block{Type: "PRIVATE KEY", Bytes: keyDER}),
		pem.EncodeToMemory(&pem.Block{Type: "CERTIFICATE", Bytes: cert.Certificate[0]})...)

	rec := &localAPIRecorder{}
	api := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		// The accept-time check asks WhoIs about the socket's own address, which
		// no peer chooses; the owner answers it and it is not recorded.
		if r.URL.Path == "/localapi/v0/whois" {
			_ = json.NewEncoder(w).Encode(ownerWho())
			return
		}
		rec.mu.Lock()
		rec.seen = append(rec.seen, r.Method+" "+r.URL.Path+"?"+r.URL.RawQuery)
		rec.mu.Unlock()
		if r.URL.Path == "/localapi/v0/cert/"+testDNSName && r.URL.RawQuery == "type=pair&min_validity=0s" {
			_, _ = w.Write(pair)
			return
		}
		http.Error(w, "recorder", http.StatusInternalServerError)
	}))
	t.Cleanup(api.Close)
	lc := &local.Client{OmitAuth: true, Dial: func(ctx context.Context, network, addr string) (net.Conn, error) {
		var d net.Dialer
		return d.DialContext(ctx, "tcp", api.Listener.Addr().String())
	}}
	return lc, rec
}

const certRequest = "GET /localapi/v0/cert/" + testDNSName + "?type=pair&min_validity=0s"

// The server name of a ClientHello is a tailnet peer's input and the handshake
// runs before WhoIs: nothing but the node's own name may reach the LocalAPI.
func TestCertificateForAnswersOnlyToTheNodesName(t *testing.T) {
	lc, rec := newLocalAPIRecorder(t)
	name := testDNSName
	get := certificateFor(lc, func() string { return name })

	for _, serverName := range []string{
		testDNSName + "?type=pair&min_validity=2400h#",
		testDNSName + "?min_validity=2400h",
		testDNSName + "/../../v0/status#",
		"a.b/../../v0/status#",
		"yuzora", // a bare label would make lc ask for the status first
		"",
		"other.tail1234.ts.net",
		testDNSName + ".",
		testDNSName + ".evil.example",
		"x" + testDNSName,
		"yuzora.tail1234.tſ.net", // Unicode case folding would match the long s
	} {
		cert, err := get(&tls.ClientHelloInfo{ServerName: serverName})
		if cert != nil || err == nil {
			t.Errorf("server name %q was answered", serverName)
		}
	}
	if seen := rec.requests(); len(seen) != 0 {
		t.Fatalf("a foreign server name reached the LocalAPI: %q", seen)
	}

	for _, serverName := range []string{testDNSName, "YUZORA.Tail1234.TS.net"} {
		cert, err := get(&tls.ClientHelloInfo{ServerName: serverName})
		if err != nil || cert == nil {
			t.Fatalf("server name %q: %v", serverName, err)
		}
	}
	// The LocalAPI only ever sees the node's own spelling of the name.
	if seen := rec.requests(); len(seen) != 2 || seen[0] != certRequest || seen[1] != certRequest {
		t.Fatalf("LocalAPI requests: %q", seen)
	}

	// The name follows the node: after a rename the old one is foreign.
	name = "renamed.tail1234.ts.net"
	if cert, err := get(&tls.ClientHelloInfo{ServerName: testDNSName}); cert != nil || err == nil {
		t.Error("the old name was answered after the node's name changed")
	}
	name = ""
	if cert, err := get(&tls.ClientHelloInfo{ServerName: testDNSName}); cert != nil || err == nil {
		t.Error("a name was answered while the node has none")
	}
	if seen := rec.requests(); len(seen) != 2 {
		t.Fatalf("LocalAPI requests after the rename: %q", seen)
	}
}

// certCallRecorder counts what reaches lc.GetCertificate, the one call that
// would go on to the LocalAPI.
type certCallRecorder struct {
	*fakeLocal
	names []string
}

func (c *certCallRecorder) GetCertificate(hi *tls.ClientHelloInfo) (*tls.Certificate, error) {
	c.names = append(c.names, hi.ServerName)
	return c.fakeLocal.GetCertificate(hi)
}

func TestCertificateForDoesNotCallOutWithoutAName(t *testing.T) {
	lc := &certCallRecorder{fakeLocal: &fakeLocal{}}
	name := ""
	get := certificateFor(lc, func() string { return name })
	// An empty server name equals an empty node name, which still is no name.
	for _, serverName := range []string{"", testDNSName} {
		if cert, err := get(&tls.ClientHelloInfo{ServerName: serverName}); cert != nil || err == nil {
			t.Errorf("server name %q was answered while the node has no name", serverName)
		}
	}
	name = testDNSName
	if _, err := get(&tls.ClientHelloInfo{ServerName: "Yuzora.Tail1234.ts.net"}); err != nil {
		t.Fatal(err)
	}
	if len(lc.names) != 1 || lc.names[0] != testDNSName {
		t.Fatalf("lc.GetCertificate saw %q, want only the node's name", lc.names)
	}
}

// The same through the real TLS listener of startService.
func TestServiceHandshakeWithACraftedServerNameNeverReachesTheLocalAPI(t *testing.T) {
	lc, rec := newLocalAPIRecorder(t)
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	target, _ := url.Parse("http://127.0.0.1:9")
	emit, _ := newTestEmitter()
	svc := startService(ln, lc, func() string { return testDNSName }, target,
		serveConfig{target: target.String(), proxySecret: testSecret, requiredLogin: testLogin}, emit, discard, time.Now)
	defer svc.close()

	dial := func(serverName string) error {
		conn, err := tls.DialWithDialer(&net.Dialer{Timeout: 5 * time.Second}, "tcp", ln.Addr().String(),
			&tls.Config{ServerName: serverName, InsecureSkipVerify: true})
		if err == nil {
			_ = conn.Close()
		}
		return err
	}
	for _, serverName := range []string{
		testDNSName + "?type=pair&min_validity=2400h#",
		"yuzora",
	} {
		if err := dial(serverName); err == nil {
			t.Errorf("the handshake for %q succeeded", serverName)
		}
	}
	if seen := rec.requests(); len(seen) != 0 {
		t.Fatalf("a crafted server name reached the LocalAPI: %q", seen)
	}
	if err := dial(testDNSName); err != nil {
		t.Fatalf("the node's own name must handshake: %v", err)
	}
	if seen := rec.requests(); len(seen) != 1 || seen[0] != certRequest {
		t.Fatalf("LocalAPI requests: %q", seen)
	}
	if strings.Contains(strings.Join(rec.requests(), "\n"), "min_validity=2400h") {
		t.Fatal("the peer's query reached the LocalAPI")
	}
}

func TestNodeCurrentDNSName(t *testing.T) {
	f := newNodeFixture(t)
	if got := f.node.currentDNSName(); got != "" {
		t.Fatalf("before the node runs: %q", got)
	}
	f.local.setStatus(runningStatus())
	f.node.poll(f.ctx)
	if got := f.node.currentDNSName(); got != testDNSName {
		t.Fatalf("running: %q", got)
	}
	f.local.setStatus(&ipnstate.Status{BackendState: "NeedsLogin"})
	f.node.poll(f.ctx)
	if got := f.node.currentDNSName(); got != "" {
		t.Fatalf("after the node needs a login again: %q", got)
	}
}

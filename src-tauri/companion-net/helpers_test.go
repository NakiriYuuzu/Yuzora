package main

import (
	"bytes"
	"context"
	"crypto/ecdsa"
	"crypto/elliptic"
	"crypto/rand"
	"crypto/tls"
	"crypto/x509"
	"crypto/x509/pkix"
	"encoding/json"
	"encoding/pem"
	"io"
	"log"
	"math/big"
	"net"
	"strings"
	"sync"
	"testing"
	"time"

	"tailscale.com/client/tailscale/apitype"
	"tailscale.com/ipn/ipnstate"
	"tailscale.com/tailcfg"
)

const (
	testDNSName = "yuzora.tail1234.ts.net"
	testLogin   = "owner@example.com"
	// 32 bytes of 0x01, base64url without padding.
	testSecret = "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE"
)

var discard = log.New(io.Discard, "", 0)

// syncBuffer is an io.Writer that tests read frames back from.
type syncBuffer struct {
	mu  sync.Mutex
	buf bytes.Buffer
}

func (b *syncBuffer) Write(p []byte) (int, error) {
	b.mu.Lock()
	defer b.mu.Unlock()
	return b.buf.Write(p)
}

func (b *syncBuffer) frames(t *testing.T) []map[string]any {
	t.Helper()
	b.mu.Lock()
	defer b.mu.Unlock()
	var out []map[string]any
	for _, line := range strings.Split(strings.TrimSpace(b.buf.String()), "\n") {
		if line == "" {
			continue
		}
		var frame map[string]any
		if err := json.Unmarshal([]byte(line), &frame); err != nil {
			t.Fatalf("bad frame %q: %v", line, err)
		}
		if frame["v"] != float64(1) {
			t.Fatalf("frame without v:1: %q", line)
		}
		out = append(out, frame)
	}
	return out
}

func (b *syncBuffer) ofType(t *testing.T, typ string) []map[string]any {
	t.Helper()
	var out []map[string]any
	for _, frame := range b.frames(t) {
		if frame["type"] == typ {
			out = append(out, frame)
		}
	}
	return out
}

func (b *syncBuffer) types(t *testing.T) []string {
	t.Helper()
	var out []string
	for _, frame := range b.frames(t) {
		out = append(out, frame["type"].(string))
	}
	return out
}

func newTestEmitter() (*emitter, *syncBuffer) {
	out := &syncBuffer{}
	return &emitter{w: out, diag: discard}, out
}

func waitFor(t *testing.T, what string, cond func() bool) {
	t.Helper()
	deadline := time.Now().Add(5 * time.Second)
	for !cond() {
		if time.Now().After(deadline) {
			t.Fatalf("timed out waiting for %s", what)
		}
		time.Sleep(5 * time.Millisecond)
	}
}

func waitFrame(t *testing.T, out *syncBuffer, typ string) map[string]any {
	t.Helper()
	var found map[string]any
	waitFor(t, typ+" frame", func() bool {
		frames := out.ofType(t, typ)
		if len(frames) == 0 {
			return false
		}
		found = frames[len(frames)-1]
		return true
	})
	return found
}

// fakeLocal is a scriptable localAPI.
type fakeLocal struct {
	mu        sync.Mutex
	status    *ipnstate.Status
	statusErr error
	certPair  func(ctx context.Context, domain string) ([]byte, []byte, error)
	whois     func(ctx context.Context, remoteAddr string) (*apitype.WhoIsResponse, error)
	logout    func(ctx context.Context) error
	cert      *tls.Certificate
	certCalls int
}

func (f *fakeLocal) setStatus(st *ipnstate.Status) {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.status = st
}

func (f *fakeLocal) StatusWithoutPeers(context.Context) (*ipnstate.Status, error) {
	f.mu.Lock()
	defer f.mu.Unlock()
	return f.status, f.statusErr
}

func (f *fakeLocal) CertPair(ctx context.Context, domain string) ([]byte, []byte, error) {
	f.mu.Lock()
	f.certCalls++
	certPair := f.certPair
	f.mu.Unlock()
	if certPair == nil {
		return nil, nil, io.ErrUnexpectedEOF
	}
	return certPair(ctx, domain)
}

func (f *fakeLocal) calls() int {
	f.mu.Lock()
	defer f.mu.Unlock()
	return f.certCalls
}

func (f *fakeLocal) GetCertificate(*tls.ClientHelloInfo) (*tls.Certificate, error) {
	return f.cert, nil
}

func (f *fakeLocal) WhoIs(ctx context.Context, remoteAddr string) (*apitype.WhoIsResponse, error) {
	if f.whois == nil {
		return ownerWho(), nil
	}
	return f.whois(ctx, remoteAddr)
}

func (f *fakeLocal) Logout(ctx context.Context) error {
	if f.logout == nil {
		return nil
	}
	return f.logout(ctx)
}

// fakeTailnet is a scriptable tailnet.
type fakeTailnet struct {
	local     *fakeLocal
	startErr  error
	listenErr error
	mu        sync.Mutex
	listener  net.Listener
	closed    chan struct{}
	closeOnce sync.Once
}

func newFakeTailnet(local *fakeLocal) *fakeTailnet {
	return &fakeTailnet{local: local, closed: make(chan struct{})}
}

func (f *fakeTailnet) Start() error { return f.startErr }

func (f *fakeTailnet) LocalClient() (localAPI, error) { return f.local, nil }

func (f *fakeTailnet) Listen(network, addr string) (net.Listener, error) {
	if f.listenErr != nil {
		return nil, f.listenErr
	}
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	f.mu.Lock()
	f.listener = ln
	f.mu.Unlock()
	return ln, err
}

func (f *fakeTailnet) listenAddr() string {
	f.mu.Lock()
	defer f.mu.Unlock()
	return f.listener.Addr().String()
}

func (f *fakeTailnet) Close() error {
	f.closeOnce.Do(func() { close(f.closed) })
	return nil
}

func runningStatus() *ipnstate.Status {
	return &ipnstate.Status{
		BackendState: backendRunning,
		Self: &ipnstate.PeerStatus{
			DNSName: testDNSName + ".",
			UserID:  7,
			CapMap:  tailcfg.NodeCapMap{tailcfg.CapabilityHTTPS: nil},
		},
		User:           map[tailcfg.UserID]tailcfg.UserProfile{7: {LoginName: testLogin, DisplayName: "Owner"}},
		CertDomains:    []string{testDNSName},
		CurrentTailnet: &ipnstate.TailnetStatus{MagicDNSEnabled: true},
	}
}

func whoFor(login string, tags ...string) *apitype.WhoIsResponse {
	return &apitype.WhoIsResponse{
		Node:        &tailcfg.Node{ComputedName: "phone", Tags: tags},
		UserProfile: &tailcfg.UserProfile{LoginName: login},
	}
}

func ownerWho() *apitype.WhoIsResponse { return whoFor(testLogin) }

// selfSigned returns a certificate for dnsName and its PEM chain.
func selfSigned(t *testing.T, dnsName string, notAfter time.Time) (*tls.Certificate, []byte) {
	t.Helper()
	key, err := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	if err != nil {
		t.Fatal(err)
	}
	template := &x509.Certificate{
		SerialNumber: big.NewInt(1),
		Subject:      pkix.Name{CommonName: dnsName},
		DNSNames:     []string{dnsName},
		NotBefore:    time.Now().Add(-time.Hour),
		NotAfter:     notAfter,
	}
	der, err := x509.CreateCertificate(rand.Reader, template, template, &key.PublicKey, key)
	if err != nil {
		t.Fatal(err)
	}
	return &tls.Certificate{Certificate: [][]byte{der}, PrivateKey: key},
		pem.EncodeToMemory(&pem.Block{Type: "CERTIFICATE", Bytes: der})
}

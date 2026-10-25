// Spike N1: an embedded Tailscale node (tsnet) in place of `tailscale serve`.
//
// Throwaway, not product code. It answers: how big the binary is, how login
// works without the Tailscale app, how long the first certificate takes, and
// whether WhoIs on the peer address can replace the Serve identity header.
//
// It behaves like Serve in front of the Companion gateway: TLS on the node's
// ts.net name, strips client Tailscale-* headers, and injects
// Tailscale-User-Login/Name from WhoIs, never for tagged nodes. Events go to
// stdout as JSON lines; tsnet's own logs go to <dir>/tsnet.log.
package main

import (
	"context"
	"crypto/tls"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"log"
	"mime"
	"net/http"
	"net/http/httputil"
	"net/url"
	"os"
	"os/signal"
	"path/filepath"
	"sort"
	"strings"
	"sync"
	"syscall"
	"time"

	"tailscale.com/client/local"
	"tailscale.com/tsnet"
)

var started = time.Now()

func emit(event string, fields map[string]any) {
	if fields == nil {
		fields = map[string]any{}
	}
	fields["event"] = event
	fields["ms"] = time.Since(started).Milliseconds()
	line, _ := json.Marshal(fields)
	fmt.Println(string(line))
}

func main() {
	dir := flag.String("dir", "/tmp/yzts/state", "tsnet state directory (holds the node key)")
	hostname := flag.String("hostname", "yuzora-spike", "tailnet machine name")
	listen := flag.String("listen", ":8443", "tailnet address to serve TLS on")
	target := flag.String("target", "http://127.0.0.1:47110", "gateway to proxy to")
	statusOut := flag.String("status-out", "", "write the node's status JSON here once running (0600)")
	logout := flag.Bool("logout", false, "log the node out and exit")
	selftestEvery := flag.Duration("selftest", 0, "dial our own ts.net name over the tailnet at this interval until it succeeds (0: off)")
	flag.Parse()

	if err := os.MkdirAll(*dir, 0o700); err != nil {
		log.Fatal(err)
	}
	logFile, err := os.OpenFile(filepath.Join(*dir, "tsnet.log"), os.O_CREATE|os.O_WRONLY|os.O_APPEND, 0o600)
	if err != nil {
		log.Fatal(err)
	}
	srv := &tsnet.Server{
		Dir:      *dir,
		Hostname: *hostname,
		AuthKey:  os.Getenv("TS_AUTHKEY"),
		Logf:     log.New(logFile, "", log.LstdFlags|log.Lmicroseconds).Printf,
		UserLogf: func(format string, args ...any) {
			emit("user-log", map[string]any{"text": fmt.Sprintf(format, args...)})
		},
	}
	defer srv.Close()

	ctx, cancel := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer cancel()

	emit("start", map[string]any{"hostname": *hostname, "dir": *dir})
	if err := srv.Start(); err != nil {
		emit("error", map[string]any{"stage": "start", "error": err.Error()})
		os.Exit(1)
	}
	lc, err := srv.LocalClient()
	if err != nil {
		emit("error", map[string]any{"stage": "local-client", "error": err.Error()})
		os.Exit(1)
	}
	if *logout {
		err := lc.Logout(ctx)
		emit("logout", map[string]any{"error": errString(err)})
		return
	}

	go watchLogin(ctx, lc)
	status, err := srv.Up(ctx)
	if err != nil {
		emit("error", map[string]any{"stage": "up", "error": err.Error()})
		os.Exit(1)
	}
	self := map[string]any{"backendState": status.BackendState, "dnsName": status.Self.DNSName, "certDomains": srv.CertDomains(), "tailscaleIPs": status.TailscaleIPs}
	if status.CurrentTailnet != nil {
		self["magicDNS"] = status.CurrentTailnet.MagicDNSEnabled
	}
	if user, ok := status.User[status.Self.UserID]; ok {
		self["login"] = user.LoginName
	}
	emit("running", self)
	go watchStatus(ctx, lc, *statusOut)

	ln, err := srv.Listen("tcp", *listen)
	if err != nil {
		emit("error", map[string]any{"stage": "listen", "error": err.Error()})
		os.Exit(1)
	}
	upstream, err := url.Parse(*target)
	if err != nil {
		log.Fatal(err)
	}
	server := &http.Server{
		Handler:           handler(lc, upstream),
		ReadHeaderTimeout: 10 * time.Second,
		ErrorLog:          log.New(logFile, "http: ", log.LstdFlags),
	}
	go func() {
		<-ctx.Done()
		shutdown, done := context.WithTimeout(context.Background(), 3*time.Second)
		defer done()
		_ = server.Shutdown(shutdown)
	}()
	tlsLn := tls.NewListener(ln, &tls.Config{GetCertificate: timedCertificate(lc), NextProtos: []string{"http/1.1"}})
	if *selftestEvery > 0 {
		go selftest(ctx, srv, strings.TrimSuffix(status.Self.DNSName, "."), portSuffix(*listen), *selftestEvery)
	}
	emit("listening", map[string]any{"addr": *listen, "url": "https://" + strings.TrimSuffix(status.Self.DNSName, ".") + portSuffix(*listen) + "/"})
	if err := server.Serve(tlsLn); err != nil && !errors.Is(err, http.ErrServerClosed) {
		emit("error", map[string]any{"stage": "serve", "error": err.Error()})
	}
}

// watchLogin reports the login URL (interactive login, no auth key) and state changes.
func watchLogin(ctx context.Context, lc *local.Client) {
	var lastURL, lastState string
	for ctx.Err() == nil {
		if st, err := lc.StatusWithoutPeers(ctx); err == nil {
			if st.AuthURL != "" && st.AuthURL != lastURL {
				lastURL = st.AuthURL
				emit("login-required", map[string]any{"authURL": st.AuthURL})
			}
			if st.BackendState != lastState {
				lastState = st.BackendState
				emit("state", map[string]any{"backendState": st.BackendState})
				if lastState == "Running" {
					return
				}
			}
		}
		time.Sleep(500 * time.Millisecond)
	}
}

// selftest fetches /__spike/whois from our own name through the tailnet, with
// the system roots, as a phone would: it proves the public certificate and WhoIs.
func selftest(ctx context.Context, srv *tsnet.Server, host, port string, every time.Duration) {
	client := &http.Client{
		Timeout:   3 * time.Minute,
		Transport: &http.Transport{DialContext: srv.Dial, TLSClientConfig: &tls.Config{ServerName: host}},
	}
	for attempt := 1; ctx.Err() == nil; attempt++ {
		begin := time.Now()
		resp, err := client.Get("https://" + host + port + "/__spike/whois")
		fields := map[string]any{"attempt": attempt, "requestMs": time.Since(begin).Milliseconds(), "error": errString(err)}
		if err == nil {
			var body map[string]any
			_ = json.NewDecoder(resp.Body).Decode(&body)
			resp.Body.Close()
			fields["status"] = resp.StatusCode
			fields["whois"] = body
			if resp.TLS != nil && len(resp.TLS.PeerCertificates) > 0 {
				cert := resp.TLS.PeerCertificates[0]
				fields["issuer"] = cert.Issuer.CommonName
				fields["notAfter"] = cert.NotAfter.Format(time.RFC3339)
			}
		}
		emit("selftest", fields)
		if err == nil {
			return
		}
		select {
		case <-ctx.Done():
		case <-time.After(every):
		}
	}
}

// watchStatus keeps the status file current and reports HTTPS readiness and
// which peers are online (OS and tagged only), so the spike sees the phone join.
func watchStatus(ctx context.Context, lc *local.Client, out string) {
	var last string
	for ctx.Err() == nil {
		if st, err := lc.Status(ctx); err == nil {
			_, https := st.Self.CapMap["https"]
			var peers []string
			for _, peer := range st.Peer {
				if peer.Online {
					peers = append(peers, fmt.Sprintf("%s tagged=%v", peer.OS, peer.Tags != nil && peer.Tags.Len() > 0))
				}
			}
			sort.Strings(peers)
			summary := fmt.Sprint(https, st.CertDomains, peers)
			if summary != last {
				last = summary
				emit("status", map[string]any{"httpsCap": https, "certDomains": st.CertDomains, "onlinePeers": peers})
			}
			if out != "" {
				data, _ := json.MarshalIndent(st, "", "  ")
				tmp := out + ".tmp"
				if err := os.WriteFile(tmp, data, 0o600); err == nil {
					_ = os.Rename(tmp, out)
				}
			}
		}
		select {
		case <-ctx.Done():
		case <-time.After(3 * time.Second):
		}
	}
}

// timedCertificate reports how long each new certificate fetch takes.
func timedCertificate(lc *local.Client) func(*tls.ClientHelloInfo) (*tls.Certificate, error) {
	var mu sync.Mutex
	seen := map[string]bool{}
	return func(hello *tls.ClientHelloInfo) (*tls.Certificate, error) {
		begin := time.Now()
		cert, err := lc.GetCertificate(hello)
		mu.Lock()
		first := !seen[hello.ServerName]
		seen[hello.ServerName] = true
		mu.Unlock()
		if first || err != nil {
			emit("certificate", map[string]any{"serverName": hello.ServerName, "fetchMs": time.Since(begin).Milliseconds(), "error": errString(err)})
		}
		return cert, err
	}
}

type identity struct {
	login, name string
	tagged      bool
	node        string
}

type identityKey struct{}

func handler(lc *local.Client, upstream *url.URL) http.Handler {
	var mu sync.Mutex
	seen := map[string]bool{}
	proxy := &httputil.ReverseProxy{
		Rewrite: func(pr *httputil.ProxyRequest) {
			pr.SetURL(upstream)
			pr.Out.Host = pr.In.Host
			for name := range pr.Out.Header {
				if strings.HasPrefix(strings.ToLower(name), "tailscale-") {
					pr.Out.Header.Del(name)
				}
			}
			who, _ := pr.In.Context().Value(identityKey{}).(identity)
			if who.login != "" && !who.tagged {
				pr.Out.Header.Set("Tailscale-User-Login", who.login)
				pr.Out.Header.Set("Tailscale-User-Name", encodeHeader(who.name))
			}
		},
		ErrorLog: log.New(os.Stderr, "proxy: ", 0),
	}
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		begin := time.Now()
		who, err := lc.WhoIs(r.Context(), r.RemoteAddr)
		if err != nil {
			emit("whois", map[string]any{"error": err.Error()})
			http.Error(w, "unknown peer", http.StatusForbidden)
			return
		}
		id := identity{tagged: who.Node.IsTagged(), node: who.Node.ComputedName}
		if who.UserProfile != nil {
			id.login, id.name = who.UserProfile.LoginName, who.UserProfile.DisplayName
		}
		mu.Lock()
		first := !seen[id.node]
		seen[id.node] = true
		mu.Unlock()
		if first {
			emit("whois", map[string]any{"node": id.node, "login": id.login, "tagged": id.tagged, "lookupMs": time.Since(begin).Milliseconds()})
		}
		if r.URL.Path == "/__spike/whois" {
			w.Header().Set("Content-Type", "application/json")
			_ = json.NewEncoder(w).Encode(map[string]any{"node": id.node, "login": id.login, "name": id.name, "tagged": id.tagged, "os": who.Node.Hostinfo.OS()})
			return
		}
		proxy.ServeHTTP(w, r.WithContext(context.WithValue(r.Context(), identityKey{}, id)))
	})
}

// encodeHeader matches Serve: RFC 2047 only when the value is not plain ASCII.
func encodeHeader(value string) string {
	for _, r := range value {
		if r > 0x7e || r < 0x20 {
			return mime.QEncoding.Encode("utf-8", value)
		}
	}
	return value
}

func portSuffix(listen string) string {
	if strings.HasSuffix(listen, ":443") {
		return ""
	}
	return listen[strings.LastIndex(listen, ":"):]
}

func errString(err error) any {
	if err == nil {
		return nil
	}
	return err.Error()
}

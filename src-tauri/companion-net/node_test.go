package main

import (
	"bufio"
	"context"
	"crypto/tls"
	"errors"
	"io"
	"net"
	"net/http"
	"sync"
	"testing"
	"time"

	"tailscale.com/ipn/ipnstate"
)

type nodeFixture struct {
	node  *node
	local *fakeLocal
	ts    *fakeTailnet
	out   *syncBuffer
	ctx   context.Context
}

func newNodeFixture(t *testing.T) *nodeFixture {
	t.Helper()
	ctx, cancel := context.WithCancel(context.Background())
	t.Cleanup(cancel)
	emit, out := newTestEmitter()
	local := &fakeLocal{}
	ts := newFakeTailnet(local)
	return &nodeFixture{node: newNode(emit, discard, ts, local), local: local, ts: ts, out: out, ctx: ctx}
}

func serveCommand(target string) command {
	return command{Type: "serve", Target: target, ProxySecret: testSecret, RequiredLogin: testLogin}
}

func TestInfoFromStatus(t *testing.T) {
	info, ok := infoFromStatus(runningStatus())
	want := runningInfo{dnsName: testDNSName, login: testLogin, displayName: "Owner", httpsCapable: true, magicDNS: true}
	if !ok || info != want {
		t.Fatalf("got %+v (%v), want %+v", info, ok, want)
	}

	variant := func(edit func(*ipnstate.Status)) *ipnstate.Status {
		st := runningStatus()
		edit(st)
		return st
	}
	tests := []struct {
		name   string
		status *ipnstate.Status
		check  func(runningInfo) bool
	}{
		{"no https capability", variant(func(s *ipnstate.Status) { s.Self.CapMap = nil }),
			func(i runningInfo) bool { return !i.httpsCapable && i.dnsName == testDNSName }},
		{"name not in cert domains", variant(func(s *ipnstate.Status) { s.CertDomains = []string{"other.tail1234.ts.net"} }),
			func(i runningInfo) bool { return !i.httpsCapable }},
		{"no cert domains", variant(func(s *ipnstate.Status) { s.CertDomains = nil }),
			func(i runningInfo) bool { return !i.httpsCapable }},
		{"case and trailing dot", variant(func(s *ipnstate.Status) {
			s.Self.DNSName = "Yuzora.Tail1234.TS.net."
			s.CertDomains = []string{"YUZORA.tail1234.ts.net"}
		}), func(i runningInfo) bool { return i.httpsCapable && i.dnsName == testDNSName }},
		{"no tailnet", variant(func(s *ipnstate.Status) { s.CurrentTailnet = nil }),
			func(i runningInfo) bool { return !i.magicDNS }},
		{"owner unknown", variant(func(s *ipnstate.Status) { s.User = nil }),
			func(i runningInfo) bool { return i.login == "" && i.displayName == "" }},
	}
	for _, tc := range tests {
		if info, ok := infoFromStatus(tc.status); !ok || !tc.check(info) {
			t.Errorf("%s: %+v (%v)", tc.name, info, ok)
		}
	}

	for name, st := range map[string]*ipnstate.Status{
		"no self":      variant(func(s *ipnstate.Status) { s.Self = nil }),
		"no self name": variant(func(s *ipnstate.Status) { s.Self.DNSName = "" }),
	} {
		if _, ok := infoFromStatus(st); ok {
			t.Errorf("%s: a status without the node's own name must not report running", name)
		}
	}
}

func TestPollReportsStateLoginAndRunning(t *testing.T) {
	f := newNodeFixture(t)
	f.local.certPair = func(ctx context.Context, _ string) ([]byte, []byte, error) {
		<-ctx.Done()
		return nil, nil, ctx.Err()
	}

	f.local.setStatus(&ipnstate.Status{BackendState: "NoState"})
	if f.node.poll(f.ctx) {
		t.Fatal("NoState is not running")
	}
	f.local.setStatus(&ipnstate.Status{BackendState: "NeedsLogin", AuthURL: "https://login.example/a"})
	f.node.poll(f.ctx)
	f.node.poll(f.ctx) // same URL, same state: nothing new
	f.local.setStatus(&ipnstate.Status{BackendState: "NeedsLogin", AuthURL: "https://login.example/b"})
	f.node.poll(f.ctx)
	f.local.setStatus(&ipnstate.Status{BackendState: "Starting"})
	f.node.poll(f.ctx)

	if got := f.out.types(t); len(got) != 5 || got[0] != "state" || got[1] != "state" || got[2] != "login-required" ||
		got[3] != "login-required" || got[4] != "state" {
		t.Fatalf("frames: %v", got)
	}
	urls := f.out.ofType(t, "login-required")
	if urls[0]["authUrl"] != "https://login.example/a" || urls[1]["authUrl"] != "https://login.example/b" {
		t.Errorf("auth URLs: %v", urls)
	}

	f.local.setStatus(runningStatus())
	if !f.node.poll(f.ctx) {
		t.Fatal("Running must be reported as running")
	}
	f.node.poll(f.ctx) // unchanged: no repeat
	running := f.out.ofType(t, "running")
	if len(running) != 1 {
		t.Fatalf("running frames: %v", running)
	}
	r := running[0]
	if r["dnsName"] != testDNSName || r["login"] != testLogin || r["displayName"] != "Owner" ||
		r["httpsCapable"] != true || r["magicDns"] != true {
		t.Errorf("running frame: %v", r)
	}
	if states := f.out.ofType(t, "state"); states[len(states)-1]["backendState"] != "Running" {
		t.Errorf("last state: %v", states)
	}

	changed := runningStatus()
	changed.Self.CapMap = nil
	f.local.setStatus(changed)
	f.node.poll(f.ctx)
	running = f.out.ofType(t, "running")
	if len(running) != 2 || running[1]["httpsCapable"] != false {
		t.Errorf("a change must be reported again: %v", running)
	}
}

func TestStateCarriesThePendingMachineNameWhileItWaitsForApproval(t *testing.T) {
	f := newNodeFixture(t)
	pendingStatus := func(dnsName string) *ipnstate.Status {
		st := &ipnstate.Status{BackendState: "NeedsMachineAuth"}
		if dnsName != "" {
			st.Self = &ipnstate.PeerStatus{DNSName: dnsName}
		}
		return st
	}
	states := func() []map[string]any { return f.out.ofType(t, "state") }

	// Registered, and the node knows its name: it comes with the state, as the
	// running event has it (lower case, no trailing dot).
	f.local.setStatus(pendingStatus("Yuzora-1.tail1234.ts.net."))
	if f.node.poll(f.ctx) {
		t.Fatal("NeedsMachineAuth is not running")
	}
	f.node.poll(f.ctx) // unchanged: nothing new
	got := states()
	if len(got) != 1 || got[0]["backendState"] != "NeedsMachineAuth" || got[0]["dnsName"] != "yuzora-1.tail1234.ts.net" {
		t.Fatalf("state frames: %v", got)
	}

	// Other states carry no name, even when the node has one.
	st := runningStatus()
	st.BackendState = "Starting"
	f.local.setStatus(st)
	f.node.poll(f.ctx)
	if got = states(); len(got) != 2 || got[1]["backendState"] != "Starting" {
		t.Fatalf("state frames: %v", got)
	} else if _, has := got[1]["dnsName"]; has {
		t.Errorf("only NeedsMachineAuth names the machine: %v", got[1])
	}

	// Without a name there is no field; when the name arrives later the state
	// is said again with it.
	f.local.setStatus(pendingStatus(""))
	f.node.poll(f.ctx)
	if got = states(); len(got) != 3 || got[2]["backendState"] != "NeedsMachineAuth" {
		t.Fatalf("state frames: %v", got)
	} else if _, has := got[2]["dnsName"]; has {
		t.Errorf("no name is known yet: %v", got[2])
	}
	f.local.setStatus(pendingStatus("yuzora-1.tail1234.ts.net."))
	f.node.poll(f.ctx)
	if got = states(); len(got) != 4 || got[3]["backendState"] != "NeedsMachineAuth" || got[3]["dnsName"] != "yuzora-1.tail1234.ts.net" {
		t.Fatalf("state frames: %v", got)
	}
}

func TestRunningIsReportedAgainWhenTheNodeRunsAfterNeedingALogin(t *testing.T) {
	f := newNodeFixture(t)
	cancelled := make(chan struct{}, 2)
	f.local.certPair = func(ctx context.Context, _ string) ([]byte, []byte, error) {
		<-ctx.Done()
		cancelled <- struct{}{}
		return nil, nil, ctx.Err()
	}
	f.local.setStatus(runningStatus())
	f.node.poll(f.ctx)
	waitFor(t, "first certificate request", func() bool { return f.local.calls() == 1 })

	// The key expired: the node asks for a login, and its certificate loop ends.
	f.local.setStatus(&ipnstate.Status{BackendState: "NeedsLogin", AuthURL: "https://login.example/a"})
	if f.node.poll(f.ctx) {
		t.Fatal("NeedsLogin is not running")
	}
	select {
	case <-cancelled:
	case <-time.After(5 * time.Second):
		t.Fatal("the certificate loop kept running while the node needed a login")
	}

	// Same process, same name: the desktop forgot the node at the login, so
	// the running frame has to come again.
	f.local.setStatus(runningStatus())
	if !f.node.poll(f.ctx) {
		t.Fatal("Running must be reported as running")
	}
	running := f.out.ofType(t, "running")
	if len(running) != 2 || running[1]["dnsName"] != testDNSName || running[1]["login"] != testLogin ||
		running[1]["httpsCapable"] != true {
		t.Fatalf("running frames: %v", running)
	}
	if states := f.out.ofType(t, "state"); len(states) != 3 || states[1]["backendState"] != "NeedsLogin" ||
		states[2]["backendState"] != "Running" {
		t.Errorf("state frames: %v", states)
	}
	waitFor(t, "a certificate request after the login", func() bool { return f.local.calls() == 2 })
	f.node.poll(f.ctx) // unchanged again: no repeat
	if got := f.out.ofType(t, "running"); len(got) != 2 {
		t.Errorf("running repeated without a change: %v", got)
	}
}

func TestPollSurvivesStatusErrors(t *testing.T) {
	f := newNodeFixture(t)
	f.local.statusErr = errors.New("local api not ready")
	if f.node.poll(f.ctx) || len(f.out.frames(t)) != 0 {
		t.Fatal("a failed poll must report nothing")
	}
	f.local.statusErr = nil
	f.local.setStatus(&ipnstate.Status{BackendState: "Starting"})
	f.node.poll(f.ctx)
	if got := f.out.types(t); len(got) != 1 || got[0] != "state" {
		t.Fatalf("frames after recovery: %v", got)
	}
}

func TestCertLoopRetrySchedule(t *testing.T) {
	f := newNodeFixture(t)
	notAfter := time.Now().Add(90 * 24 * time.Hour).UTC().Truncate(time.Second)
	_, certPEM := selfSigned(t, testDNSName, notAfter)
	attempts := 0
	f.local.certPair = func(_ context.Context, domain string) ([]byte, []byte, error) {
		if domain != testDNSName {
			t.Errorf("certificate requested for %q", domain)
		}
		attempts++
		if attempts <= 5 {
			return nil, nil, errors.New("your Tailscale account does not support getting TLS certs")
		}
		return certPEM, nil, nil
	}
	var waits []time.Duration
	f.node.sleep = func(_ context.Context, d time.Duration) error {
		waits = append(waits, d)
		return nil
	}

	f.node.certLoop(f.ctx, testDNSName)

	wantWaits := []time.Duration{30 * time.Second, 60 * time.Second, 120 * time.Second, 300 * time.Second, 300 * time.Second}
	if len(waits) != len(wantWaits) {
		t.Fatalf("waits: %v", waits)
	}
	for i := range wantWaits {
		if waits[i] != wantWaits[i] {
			t.Errorf("wait %d = %v, want %v", i, waits[i], wantWaits[i])
		}
	}
	certs := f.out.ofType(t, "cert")
	if len(certs) != 12 {
		t.Fatalf("cert frames: %d", len(certs))
	}
	for i, frame := range certs {
		wantState := []string{"pending", "error"}[i%2]
		if i == 10 {
			wantState = "pending"
		}
		if i == 11 {
			wantState = "ready"
		}
		if frame["state"] != wantState {
			t.Fatalf("cert frame %d is %v, want %s", i, frame["state"], wantState)
		}
		switch wantState {
		case "ready":
			if frame["notAfter"] != notAfter.Format(time.RFC3339) || frame["error"] != nil {
				t.Errorf("ready frame: %v", frame)
			}
		case "error":
			if frame["notAfter"] != nil || frame["error"] == nil {
				t.Errorf("error frame: %v", frame)
			}
		default:
			if frame["notAfter"] != nil || frame["error"] != nil {
				t.Errorf("pending frame: %v", frame)
			}
		}
	}
	if !f.node.certReady {
		t.Error("the loop must mark the certificate ready")
	}
}

func TestCertRetryDelayAfterTheSchedule(t *testing.T) {
	for attempt, want := range []time.Duration{30, 60, 120, 300, 300, 300} {
		if got := certRetryDelay(attempt); got != want*time.Second {
			t.Errorf("attempt %d: %v, want %vs", attempt, got, int64(want))
		}
	}
}

func TestCertLoopStopsWhenCancelledWhileWaiting(t *testing.T) {
	f := newNodeFixture(t)
	ctx, cancel := context.WithCancel(f.ctx)
	f.local.certPair = func(context.Context, string) ([]byte, []byte, error) { return nil, nil, errors.New("rate limited") }
	f.node.sleep = func(ctx context.Context, _ time.Duration) error {
		cancel()
		return ctx.Err()
	}
	done := make(chan struct{})
	go func() { f.node.certLoop(ctx, testDNSName); close(done) }()
	select {
	case <-done:
	case <-time.After(5 * time.Second):
		t.Fatal("the loop kept running after cancellation")
	}
	if f.node.certReady {
		t.Error("a cancelled loop must not be ready")
	}
}

func TestCertNotAfter(t *testing.T) {
	notAfter := time.Date(2027, 1, 2, 3, 4, 5, 0, time.UTC)
	_, certPEM := selfSigned(t, testDNSName, notAfter)
	got, err := certNotAfter(append([]byte("-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----\n"), certPEM...))
	if err != nil || !got.Equal(notAfter) {
		t.Fatalf("got %v, %v", got, err)
	}
	for _, bad := range []string{"", "not pem", "-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n"} {
		if _, err := certNotAfter([]byte(bad)); err == nil {
			t.Errorf("%q accepted", bad)
		}
	}
}

func TestNoCertificateRequestWithoutHTTPSCapability(t *testing.T) {
	f := newNodeFixture(t)
	f.local.certPair = func(context.Context, string) ([]byte, []byte, error) {
		return nil, nil, errors.New("must not be called")
	}
	for name, edit := range map[string]func(*ipnstate.Status){
		"no capability":      func(s *ipnstate.Status) { s.Self.CapMap = nil },
		"name not in domain": func(s *ipnstate.Status) { s.CertDomains = nil },
	} {
		st := runningStatus()
		edit(st)
		f.local.setStatus(st)
		f.node.poll(f.ctx)
		if f.node.certFor != "" || f.local.calls() != 0 {
			t.Errorf("%s: a certificate was requested", name)
		}
	}
	if got := f.out.ofType(t, "cert"); len(got) != 0 {
		t.Errorf("cert frames without HTTPS capability: %v", got)
	}
}

func TestCertLoopFollowsCapabilityAndName(t *testing.T) {
	f := newNodeFixture(t)
	var mu sync.Mutex
	var domains []string
	cancelled := make(chan string, 4)
	f.local.certPair = func(ctx context.Context, domain string) ([]byte, []byte, error) {
		mu.Lock()
		domains = append(domains, domain)
		mu.Unlock()
		<-ctx.Done()
		cancelled <- domain
		return nil, nil, ctx.Err()
	}
	f.local.setStatus(runningStatus())
	f.node.poll(f.ctx)
	f.node.poll(f.ctx) // a second poll must not start a second loop
	waitFor(t, "first certificate request", func() bool { return f.local.calls() == 1 })

	lost := runningStatus()
	lost.CertDomains = nil
	f.local.setStatus(lost)
	f.node.poll(f.ctx)
	if got := <-cancelled; got != testDNSName {
		t.Fatalf("cancelled %q", got)
	}

	renamed := runningStatus()
	renamed.Self.DNSName = "yuzora-1.tail1234.ts.net."
	renamed.CertDomains = []string{"yuzora-1.tail1234.ts.net"}
	f.local.setStatus(renamed)
	f.node.poll(f.ctx)
	waitFor(t, "request for the new name", func() bool { return f.local.calls() == 2 })
	mu.Lock()
	defer mu.Unlock()
	if len(domains) != 2 || domains[1] != "yuzora-1.tail1234.ts.net" {
		t.Errorf("requested domains: %v", domains)
	}
	for _, frame := range f.out.ofType(t, "cert") {
		if frame["state"] == "error" {
			t.Errorf("a cancelled attempt reported an error: %v", frame)
		}
	}
}

// readyNode brings a fixture to Running with a certificate in hand.
func readyNode(t *testing.T, f *nodeFixture) {
	t.Helper()
	cert, certPEM := selfSigned(t, testDNSName, time.Now().Add(time.Hour))
	f.local.cert = cert
	f.local.certPair = func(context.Context, string) ([]byte, []byte, error) { return certPEM, nil, nil }
	f.local.setStatus(runningStatus())
	f.node.poll(f.ctx)
	waitFor(t, "certificate", func() bool {
		f.node.mu.Lock()
		defer f.node.mu.Unlock()
		return f.node.certReady
	})
}

func lastError(t *testing.T, f *nodeFixture) string {
	t.Helper()
	errs := f.out.ofType(t, "error")
	if len(errs) == 0 {
		return ""
	}
	return errs[len(errs)-1]["code"].(string)
}

func TestServeStateMachine(t *testing.T) {
	f := newNodeFixture(t)
	good := serveCommand("http://127.0.0.1:47110")

	f.node.serve(good)
	if lastError(t, f) != "net-not-ready" {
		t.Fatalf("serve before the node runs: %v", f.out.frames(t))
	}

	bad := good
	bad.Target = "http://localhost:47110"
	f.node.serve(bad)
	if lastError(t, f) != "net-serve-invalid" {
		t.Fatalf("an invalid serve is invalid in any state: %v", f.out.frames(t))
	}

	release := make(chan struct{})
	cert, certPEM := selfSigned(t, testDNSName, time.Now().Add(time.Hour))
	f.local.cert = cert
	f.local.certPair = func(context.Context, string) ([]byte, []byte, error) { <-release; return certPEM, nil, nil }
	f.local.setStatus(runningStatus())
	f.node.poll(f.ctx)
	waitFrame(t, f.out, "running")
	f.node.serve(good)
	if errs := f.out.ofType(t, "error"); len(errs) != 3 || errs[2]["code"] != "net-not-ready" {
		t.Fatalf("serve while the certificate is pending: %v", errs)
	}
	if len(f.out.ofType(t, "serving")) != 0 {
		t.Fatal("served without a certificate")
	}

	close(release)
	waitFor(t, "cert ready", func() bool {
		for _, frame := range f.out.ofType(t, "cert") {
			if frame["state"] == "ready" {
				return true
			}
		}
		return false
	})
	f.ts.listenErr = errors.New("address in use")
	f.node.serve(good)
	if lastError(t, f) != "net-listen-failed" || f.node.svc != nil {
		t.Fatalf("listen failure: %v", f.out.frames(t))
	}

	f.ts.listenErr = nil
	f.node.serve(good)
	serving := f.out.ofType(t, "serving")
	if len(serving) != 1 || serving[0]["url"] != "https://"+testDNSName+"/" {
		t.Fatalf("serving frames: %v", serving)
	}
	t.Cleanup(f.node.stopServing)

	f.node.serve(good)
	if lastError(t, f) != "net-serve-invalid" || len(f.out.ofType(t, "serving")) != 1 {
		t.Fatalf("a second serve must be rejected: %v", f.out.frames(t))
	}

	f.node.unserve()
	if len(f.out.ofType(t, "unserved")) != 1 || f.node.svc != nil {
		t.Fatalf("unserve: %v", f.out.frames(t))
	}
	f.node.unserve() // acknowledged even when nothing is served
	if len(f.out.ofType(t, "unserved")) != 2 {
		t.Fatal("unserve must always be acknowledged")
	}
	f.node.serve(good)
	if len(f.out.ofType(t, "serving")) != 2 {
		t.Fatalf("serving again after unserve: %v", f.out.frames(t))
	}
}

func TestServeEndToEndAndUnserveClosesConnections(t *testing.T) {
	f := newNodeFixture(t)
	readyNode(t, f)
	gateway := newUpstream(t)
	f.node.serve(serveCommand(gateway.URL))
	if len(f.out.ofType(t, "serving")) != 1 {
		t.Fatalf("not serving: %v", f.out.frames(t))
	}
	t.Cleanup(f.node.stopServing)
	addr := f.ts.listenAddr()
	tlsConfig := &tls.Config{ServerName: testDNSName, InsecureSkipVerify: true, NextProtos: []string{"h2", "http/1.1"}}

	client := &http.Client{Transport: &http.Transport{TLSClientConfig: tlsConfig}}
	resp, err := client.Get("https://" + addr + "/hello")
	if err != nil {
		t.Fatal(err)
	}
	body, _ := io.ReadAll(resp.Body)
	resp.Body.Close()
	if string(body) != "gateway:/hello" || resp.ProtoMajor != 1 || resp.TLS.Version < tls.VersionTLS12 {
		t.Fatalf("response %q proto=%d tls=%x", body, resp.ProtoMajor, resp.TLS.Version)
	}
	if seen := gateway.seen(); len(seen) != 1 || seen[0].Header.Get("Yzc-Proxy-Secret") != testSecret {
		t.Fatalf("gateway requests: %v", seen)
	}

	conn, err := tls.Dial("tcp", addr, tlsConfig)
	if err != nil {
		t.Fatal(err)
	}
	defer conn.Close()
	_ = conn.SetDeadline(time.Now().Add(5 * time.Second))
	upgrade := "GET /ws HTTP/1.1\r\nHost: " + testDNSName + "\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n\r\n"
	if _, err := io.WriteString(conn, upgrade); err != nil {
		t.Fatal(err)
	}
	reader := bufio.NewReader(conn)
	if resp, err := http.ReadResponse(reader, nil); err != nil || resp.StatusCode != http.StatusSwitchingProtocols {
		t.Fatalf("upgrade: %v %v", resp, err)
	}

	f.node.unserve()

	_ = conn.SetReadDeadline(time.Now().Add(2 * time.Second))
	_, err = reader.ReadByte()
	if err == nil {
		t.Fatal("the upgraded connection delivered data after unserve")
	}
	if netErr, ok := err.(net.Error); ok && netErr.Timeout() {
		t.Fatalf("the upgraded connection survived unserve: %v", err)
	}
	if c, err := tls.Dial("tcp", addr, tlsConfig); err == nil {
		c.Close()
		t.Fatal("the listener survived unserve")
	}
}

func TestLogout(t *testing.T) {
	f := newNodeFixture(t)
	f.local.logout = func(ctx context.Context) error {
		if _, ok := ctx.Deadline(); !ok {
			t.Error("logout must be time-bounded")
		}
		return errors.New("control plane unreachable")
	}
	if f.node.logout(f.ctx) || lastError(t, f) != "net-logout-failed" || len(f.out.ofType(t, "logged-out")) != 0 {
		t.Fatalf("failed logout: %v", f.out.frames(t))
	}
	f.local.logout = nil
	if !f.node.logout(f.ctx) || len(f.out.ofType(t, "logged-out")) != 1 {
		t.Fatalf("logout: %v", f.out.frames(t))
	}
}

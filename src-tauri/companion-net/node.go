package main

import (
	"context"
	"crypto/tls"
	"crypto/x509"
	"encoding/pem"
	"errors"
	"log"
	"net"
	"net/http"
	"net/url"
	"strings"
	"sync"
	"time"

	"tailscale.com/ipn/ipnstate"
	"tailscale.com/tailcfg"
)

const (
	backendRunning       = "Running"
	backendNeedsApproval = "NeedsMachineAuth"

	pollStarting    = 500 * time.Millisecond
	pollRunning     = 5 * time.Second
	certAttemptTime = 3 * time.Minute
	logoutTimeout   = 8 * time.Second
	listenAddr      = ":443"

	// maxConns caps the connections the listener holds at once, TLS
	// handshakes and upgraded connections included. It equals the Rust
	// gateway's own 64-connection semaphore: the proxy opens one gateway
	// connection per request in flight (a WebSocket for its whole life), so
	// admitting more than the gateway serves would only queue 503s. Peers
	// that are not the owner are closed at accept time (see admitListener),
	// so they never hold a slot beyond one WhoIs lookup.
	maxConns = 64
)

// readTimeout bounds reading a request's headers and body; it matches the
// gateway's own body timeout. Without it a peer that declares a body and never
// sends it holds the connection, and its slot, for as long as it likes, even
// after the gateway has answered. It bounds neither a long poll nor a WebSocket:
// the server clears the deadline once the body has been read (at once when
// there is none) and when the proxy hijacks the connection. A variable for tests.
var readTimeout = 15 * time.Second

// runningInfo is what the running event reports; a change in any field re-emits it.
type runningInfo struct {
	dnsName      string
	login        string
	displayName  string
	httpsCapable bool
	magicDNS     bool
}

// infoFromStatus reads the node's identity from a Running status, or reports
// false while the netmap has not delivered the node's own name yet.
func infoFromStatus(st *ipnstate.Status) (runningInfo, bool) {
	if st.Self == nil || st.Self.DNSName == "" {
		return runningInfo{}, false
	}
	info := runningInfo{dnsName: strings.ToLower(strings.TrimSuffix(st.Self.DNSName, "."))}
	if owner, ok := st.User[st.Self.UserID]; ok {
		info.login, info.displayName = owner.LoginName, owner.DisplayName
	}
	if st.CurrentTailnet != nil {
		info.magicDNS = st.CurrentTailnet.MagicDNSEnabled
	}
	if st.Self.CapMap.Contains(tailcfg.CapabilityHTTPS) {
		for _, domain := range st.CertDomains {
			if strings.ToLower(strings.TrimSuffix(domain, ".")) == info.dnsName {
				info.httpsCapable = true
			}
		}
	}
	return info, true
}

// certRetryDelay is the wait after the attempt-th failed certificate request.
func certRetryDelay(attempt int) time.Duration {
	delays := []time.Duration{30 * time.Second, 60 * time.Second, 120 * time.Second}
	if attempt < len(delays) {
		return delays[attempt]
	}
	return 300 * time.Second
}

// certNotAfter returns the expiry of the first certificate in a PEM chain.
func certNotAfter(certPEM []byte) (time.Time, error) {
	for {
		var block *pem.Block
		block, certPEM = pem.Decode(certPEM)
		if block == nil {
			return time.Time{}, errors.New("no certificate in PEM")
		}
		if block.Type != "CERTIFICATE" {
			continue
		}
		leaf, err := x509.ParseCertificate(block.Bytes)
		if err != nil {
			return time.Time{}, err
		}
		return leaf.NotAfter, nil
	}
}

func sleepContext(ctx context.Context, d time.Duration) error {
	timer := time.NewTimer(d)
	defer timer.Stop()
	select {
	case <-ctx.Done():
		return ctx.Err()
	case <-timer.C:
		return nil
	}
}

// node drives one tailnet node: it reports its state, pre-issues the HTTPS
// certificate and, on request, serves the gateway to the owner's devices.
type node struct {
	emit  *emitter
	diag  *log.Logger
	ts    tailnet
	lc    localAPI
	sleep func(ctx context.Context, d time.Duration) error
	now   func() time.Time

	mu         sync.Mutex
	backend    string
	authURL    string
	pending    string // DNS name last reported with NeedsMachineAuth; "" when none
	info       *runningInfo
	certFor    string // DNS name the certificate loop works on; "" when none runs
	certCancel context.CancelFunc
	certReady  bool
	pollErr    string

	svc *service // owned by the command loop
}

func newNode(emit *emitter, diag *log.Logger, ts tailnet, lc localAPI) *node {
	return &node{emit: emit, diag: diag, ts: ts, lc: lc, sleep: sleepContext, now: time.Now}
}

// pollLoop reports the node's state until ctx ends: every 500 ms while it is
// coming up, every 5 s once it runs.
func (n *node) pollLoop(ctx context.Context) {
	for {
		interval := pollStarting
		if n.poll(ctx) {
			interval = pollRunning
		}
		if n.sleep(ctx, interval) != nil {
			return
		}
	}
}

// poll reads the status once and reports whether the node is Running with its
// own name known.
func (n *node) poll(ctx context.Context) bool {
	st, err := n.lc.StatusWithoutPeers(ctx)
	n.mu.Lock()
	defer n.mu.Unlock()
	if err != nil {
		if msg := err.Error(); ctx.Err() == nil && msg != n.pollErr {
			n.diag.Printf("status: %v", err)
			n.pollErr = msg
		}
		return false
	}
	n.pollErr = ""
	// A machine that waits for an admin's approval has its name already, and
	// the approval has to be given to that one: it says so with the state, and
	// again if the name only arrives after it.
	pending := ""
	if st.BackendState == backendNeedsApproval && st.Self != nil {
		pending = strings.ToLower(strings.TrimSuffix(st.Self.DNSName, "."))
	}
	if st.BackendState != n.backend || pending != n.pending {
		n.backend, n.pending = st.BackendState, pending
		fields := map[string]any{"backendState": st.BackendState}
		if pending != "" {
			fields["dnsName"] = pending
		}
		n.emit.send("state", fields)
	}
	if st.AuthURL != "" && st.AuthURL != n.authURL {
		n.emit.send("login-required", map[string]any{"authUrl": st.AuthURL})
	}
	n.authURL = st.AuthURL
	if st.BackendState != backendRunning {
		// Once the node runs again (after a login) it says so again: what
		// the desktop knew of it is gone with the login.
		n.info = nil
		n.stopCert()
		return false
	}
	info, ok := infoFromStatus(st)
	if !ok {
		return false
	}
	if n.info == nil || *n.info != info {
		n.info = &info
		n.emit.send("running", map[string]any{
			"dnsName":      info.dnsName,
			"login":        info.login,
			"displayName":  info.displayName,
			"httpsCapable": info.httpsCapable,
			"magicDns":     info.magicDNS,
		})
	}
	n.syncCert(ctx, info)
	return true
}

// syncCert keeps exactly one certificate loop alive while the node is HTTPS
// capable, and none otherwise. The caller holds n.mu.
func (n *node) syncCert(ctx context.Context, info runningInfo) {
	if !info.httpsCapable || n.certFor != info.dnsName {
		n.stopCert()
	}
	if !info.httpsCapable || n.certFor != "" {
		return
	}
	certCtx, cancel := context.WithCancel(ctx)
	n.certFor, n.certCancel = info.dnsName, cancel
	go n.certLoop(certCtx, info.dnsName)
}

// stopCert ends the certificate loop, if one runs. The caller holds n.mu.
func (n *node) stopCert() {
	if n.certFor != "" {
		n.certCancel()
		n.certFor, n.certCancel, n.certReady = "", nil, false
	}
}

// certLoop asks for the certificate until it succeeds, backing off between
// failures. The first issuance takes about 30 s, which is why it runs here
// and not on the first phone connection.
func (n *node) certLoop(ctx context.Context, dnsName string) {
	for attempt := 0; ; attempt++ {
		n.reportCert(ctx, "pending", nil, "")
		attemptCtx, cancel := context.WithTimeout(ctx, certAttemptTime)
		certPEM, _, err := n.lc.CertPair(attemptCtx, dnsName)
		cancel()
		var notAfter time.Time
		if err == nil {
			notAfter, err = certNotAfter(certPEM)
		}
		if err == nil {
			n.reportCert(ctx, "ready", &notAfter, "")
			return
		}
		n.reportCert(ctx, "error", nil, err.Error())
		if n.sleep(ctx, certRetryDelay(attempt)) != nil {
			return
		}
	}
}

// reportCert emits a cert event unless the loop was cancelled meanwhile.
func (n *node) reportCert(ctx context.Context, state string, notAfter *time.Time, failure string) {
	n.mu.Lock()
	defer n.mu.Unlock()
	if ctx.Err() != nil {
		return
	}
	n.certReady = state == "ready"
	fields := map[string]any{"state": state, "notAfter": nil, "error": nil}
	if notAfter != nil {
		fields["notAfter"] = notAfter.UTC().Format(time.RFC3339)
	}
	if failure != "" {
		fields["error"] = failure
	}
	n.emit.send("cert", fields)
}

// serve starts the listener once the node runs and holds a certificate.
func (n *node) serve(cmd command) {
	cfg, err := validateServe(cmd)
	if err != nil {
		n.emit.fail("net-serve-invalid", err)
		return
	}
	if n.svc != nil {
		n.emit.fail("net-serve-invalid", errors.New("already serving"))
		return
	}
	n.mu.Lock()
	ready := n.backend == backendRunning && n.info != nil && n.certReady
	var dnsName string
	if ready {
		dnsName = n.info.dnsName
	}
	n.mu.Unlock()
	if !ready {
		n.emit.fail("net-not-ready", errors.New("the node is not running with a certificate"))
		return
	}
	target, err := url.Parse(cfg.target)
	if err != nil {
		n.emit.fail("net-serve-invalid", err)
		return
	}
	ln, err := n.ts.Listen("tcp", listenAddr)
	if err != nil {
		n.emit.fail("net-listen-failed", err)
		return
	}
	n.svc = startService(ln, n.lc, n.currentDNSName, target, cfg, n.emit, n.diag, n.now)
	n.emit.send("serving", map[string]any{"url": "https://" + dnsName + "/"})
}

// currentDNSName is the node's own name, empty while the node does not run.
func (n *node) currentDNSName() string {
	n.mu.Lock()
	defer n.mu.Unlock()
	if n.info == nil {
		return ""
	}
	return n.info.dnsName
}

// unserve stops serving and acknowledges even when nothing was served, so the
// desktop's cleanup path can always wait for the acknowledgement.
func (n *node) unserve() {
	n.stopServing()
	n.emit.send("unserved", nil)
}

func (n *node) stopServing() {
	if n.svc != nil {
		n.svc.close()
		n.svc = nil
	}
}

// logout signs the node out of the tailnet. It reports whether the node is gone.
func (n *node) logout(ctx context.Context) bool {
	logoutCtx, cancel := context.WithTimeout(ctx, logoutTimeout)
	defer cancel()
	if err := n.lc.Logout(logoutCtx); err != nil {
		n.emit.fail("net-logout-failed", err)
		return false
	}
	n.emit.send("logged-out", nil)
	return true
}

// service is the listener and HTTP server in front of the gateway.
type service struct {
	server *http.Server
	conns  *connSet
	logs   *limitedLog
	done   chan struct{}
}

// certificateFor hands the LocalAPI the node's own name for any ClientHello
// that names the node, and fails every other one at once. The handshake runs
// before WhoIs, so the server name is a tailnet peer's input: lc.GetCertificate
// splices it unescaped into the LocalAPI URL, where a peer could choose the
// query (min_validity forces a new ACME order), and a bare label makes it ask
// for the status first. The comparison is ASCII-only: Unicode case folding
// would also match "ſ" and the Kelvin sign.
func certificateFor(lc localAPI, currentName func() string) func(*tls.ClientHelloInfo) (*tls.Certificate, error) {
	return func(hi *tls.ClientHelloInfo) (*tls.Certificate, error) {
		name := currentName()
		if name == "" || !sameHostName(hi.ServerName, name) {
			return nil, errors.New("the server name is not the node's name")
		}
		canonical := *hi
		canonical.ServerName = name
		return lc.GetCertificate(&canonical)
	}
}

// sameHostName compares a server name with the node's lowercase name.
func sameHostName(serverName, name string) bool {
	if len(serverName) != len(name) {
		return false
	}
	for i := 0; i < len(name); i++ {
		c := serverName[i]
		if 'A' <= c && c <= 'Z' {
			c += 'a' - 'A'
		}
		if c != name[i] {
			return false
		}
	}
	return true
}

func startService(ln net.Listener, lc localAPI, currentName func() string, target *url.URL, cfg serveConfig, emit *emitter, diag *log.Logger, now func() time.Time) *service {
	conns := &connSet{conns: map[net.Conn]struct{}{}}
	// Both loggers write one line per peer action, so they share a limit.
	logs := newLimitedLog(diag, now)
	connLog := log.New(logs, "", 0)
	reject := rejectReporter(emit, now)
	handler := newProxyHandler(lc, target, cfg, reject, connLog)
	server := &http.Server{
		Handler:           handler,
		ReadHeaderTimeout: 10 * time.Second,
		ReadTimeout:       readTimeout,
		IdleTimeout:       120 * time.Second,
		MaxHeaderBytes:    16 << 10,
		ErrorLog:          connLog,
	}
	admitted := newAdmitListener(ln, lc, cfg.requiredLogin, reject, maxConns)
	tlsListener := tls.NewListener(&trackingListener{Listener: admitted, set: conns}, &tls.Config{
		GetCertificate: certificateFor(lc, currentName),
		NextProtos:     []string{"http/1.1"},
		MinVersion:     tls.VersionTLS12,
	})
	svc := &service{server: server, conns: conns, logs: logs, done: make(chan struct{})}
	go func() {
		defer close(svc.done)
		if err := server.Serve(tlsListener); err != nil && !errors.Is(err, http.ErrServerClosed) {
			diag.Printf("serve: %v", err)
		}
	}()
	return svc
}

// close drops the listener and every connection, upgraded ones included:
// http.Server.Close leaves hijacked connections, such as WebSockets, open.
func (s *service) close() {
	_ = s.server.Close()
	s.conns.closeAll()
	<-s.done
	s.logs.close()
}

// connSet tracks accepted connections so close can end them all.
type connSet struct {
	mu     sync.Mutex
	conns  map[net.Conn]struct{}
	closed bool
}

func (c *connSet) add(conn net.Conn) bool {
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.closed {
		return false
	}
	c.conns[conn] = struct{}{}
	return true
}

func (c *connSet) remove(conn net.Conn) {
	c.mu.Lock()
	defer c.mu.Unlock()
	delete(c.conns, conn)
}

func (c *connSet) closeAll() {
	c.mu.Lock()
	c.closed = true
	conns := make([]net.Conn, 0, len(c.conns))
	for conn := range c.conns {
		conns = append(conns, conn)
	}
	c.mu.Unlock()
	for _, conn := range conns {
		_ = conn.Close()
	}
}

type trackingListener struct {
	net.Listener
	set *connSet
}

func (l *trackingListener) Accept() (net.Conn, error) {
	conn, err := l.Listener.Accept()
	if err != nil {
		return nil, err
	}
	tracked := &trackedConn{Conn: conn, set: l.set}
	if !l.set.add(tracked) {
		_ = conn.Close()
		return nil, net.ErrClosed
	}
	return tracked, nil
}

type trackedConn struct {
	net.Conn
	set  *connSet
	once sync.Once
}

func (c *trackedConn) Close() error {
	c.once.Do(func() { c.set.remove(c) })
	return c.Conn.Close()
}

// admitListener decides, before a socket reaches TLS or http.Server, whether
// it may be served at all. Every connection needs a free slot, or it is closed
// at once (waiting in Accept would let a flood stall the owner's reconnect for
// as long as the flood lasts). A connection that holds a slot is then checked
// with WhoIs, with the rules of the request handler: a tagged node, another
// user or a failed lookup (fail closed) is reported as peer.rejected and
// closed without a single TLS byte read. The lookup runs off the Accept loop,
// so a slow answer holds only that connection's slot, for at most whoisTimeout.
// Requests are still checked with WhoIs one by one: a keep-alive connection
// outlives the identity it was admitted with.
type admitListener struct {
	net.Listener
	lc     localAPI
	login  string
	reject func(peer, reason string)
	slots  chan struct{}
	ready  chan net.Conn
	errs   chan error
	ctx    context.Context
	cancel context.CancelFunc
}

func newAdmitListener(ln net.Listener, lc localAPI, login string, reject func(peer, reason string), limit int) *admitListener {
	ctx, cancel := context.WithCancel(context.Background())
	l := &admitListener{
		Listener: ln, lc: lc, login: login, reject: reject,
		slots: make(chan struct{}, limit),
		ready: make(chan net.Conn),
		errs:  make(chan error),
		ctx:   ctx, cancel: cancel,
	}
	go l.pump()
	return l
}

// pump accepts sockets until the listener closes and admits each in its own
// goroutine; those are bounded by the slots.
func (l *admitListener) pump() {
	for {
		conn, err := l.Listener.Accept()
		if err != nil {
			select {
			case l.errs <- err:
			case <-l.ctx.Done():
				return
			}
			if errors.Is(err, net.ErrClosed) {
				return
			}
			continue
		}
		select {
		case l.slots <- struct{}{}:
			go l.admit(&slotConn{Conn: conn, slots: l.slots})
		default:
			_ = conn.Close()
		}
	}
}

func (l *admitListener) admit(conn *slotConn) {
	ctx, cancel := context.WithTimeout(l.ctx, whoisTimeout)
	who, err := l.lc.WhoIs(ctx, conn.RemoteAddr().String())
	cancel()
	if l.ctx.Err() != nil {
		// The service is closing and cut the lookup short: that says nothing
		// about the peer, so nothing is reported.
		_ = conn.Close()
		return
	}
	if reason, peer := authorize(who, err, l.login); reason != "" {
		l.reject(peer, reason)
		_ = conn.Close()
		return
	}
	select {
	case l.ready <- conn:
	case <-l.ctx.Done():
		_ = conn.Close()
	}
}

func (l *admitListener) Accept() (net.Conn, error) {
	select {
	case conn := <-l.ready:
		return conn, nil
	case err := <-l.errs:
		return nil, err
	case <-l.ctx.Done():
		return nil, net.ErrClosed
	}
}

func (l *admitListener) Close() error {
	l.cancel()
	return l.Listener.Close()
}

// slotConn returns its slot when it closes, once however often it is closed:
// http.Server closes a connection it serves, a hijacker closes its own, and
// connSet.closeAll closes whatever is left.
type slotConn struct {
	net.Conn
	slots chan struct{}
	once  sync.Once
}

func (c *slotConn) Close() error {
	c.once.Do(func() { <-c.slots })
	return c.Conn.Close()
}

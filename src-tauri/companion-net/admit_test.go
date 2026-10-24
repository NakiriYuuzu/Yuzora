package main

import (
	"context"
	"crypto/tls"
	"io"
	"net"
	"net/http"
	"net/url"
	"sync/atomic"
	"testing"
	"time"

	"tailscale.com/client/tailscale/apitype"
)

func startAdmitService(t *testing.T, lc *fakeLocal) (*service, string, *syncBuffer) {
	t.Helper()
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	target, _ := url.Parse("http://127.0.0.1:9")
	emit, out := newTestEmitter()
	svc := startService(ln, lc, func() string { return testDNSName }, target,
		serveConfig{target: target.String(), proxySecret: testSecret, requiredLogin: testLogin}, emit, discard, newTestClock().read)
	t.Cleanup(svc.close)
	return svc, ln.Addr().String(), out
}

// closedByServer reports whether the server ends the connection within wait.
func closedByServer(c net.Conn, wait time.Duration) bool {
	_ = c.SetReadDeadline(time.Now().Add(wait))
	_, err := c.Read(make([]byte, 1))
	if ne, ok := err.(net.Error); ok && ne.Timeout() {
		return false
	}
	return err != nil
}

// maxConns idle connections stay open and the next one is closed at once.
func TestServiceCapsConcurrentConnections(t *testing.T) {
	_, addr, _ := startAdmitService(t, &fakeLocal{})
	conns := make([]net.Conn, 0, maxConns+1)
	for i := 0; i <= maxConns; i++ {
		c, err := net.Dial("tcp", addr)
		if err != nil {
			t.Fatal(err)
		}
		defer c.Close()
		conns = append(conns, c)
	}
	closed := make(chan int, len(conns))
	for i, c := range conns {
		go func() {
			if closedByServer(c, 500*time.Millisecond) {
				closed <- i
			}
		}()
	}
	time.Sleep(700 * time.Millisecond)
	if len(closed) != 1 {
		t.Fatalf("%d of %d connections were closed, want exactly 1", len(closed), len(conns))
	}
	// Closing one held connection frees its slot for the next peer.
	dropped := <-closed
	for i, c := range conns {
		if i != dropped {
			_ = c.Close()
			break
		}
	}
	waitFor(t, "a freed slot to admit a new connection", func() bool {
		c, err := net.Dial("tcp", addr)
		if err != nil {
			return false
		}
		defer c.Close()
		return !closedByServer(c, 200*time.Millisecond)
	})
}

// A hijacked connection (a WebSocket) keeps its slot until the hijacker closes
// it, and gives it back exactly once however often it is closed.
func TestAdmitListenerReleasesSlotOfHijackedConnection(t *testing.T) {
	raw, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	admitted := newAdmitListener(raw, &fakeLocal{}, testLogin, func(string, string) {}, 2)
	set := &connSet{conns: map[net.Conn]struct{}{}}
	hijacked := make(chan net.Conn, 1)
	server := &http.Server{Handler: http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		conn, _, err := w.(http.Hijacker).Hijack()
		if err != nil {
			t.Error(err)
			return
		}
		hijacked <- conn
	})}
	go func() { _ = server.Serve(&trackingListener{Listener: admitted, set: set}) }()
	defer server.Close()

	c, err := net.Dial("tcp", raw.Addr().String())
	if err != nil {
		t.Fatal(err)
	}
	defer c.Close()
	// A second, idle connection holds the other slot: a double release would
	// take its slot away.
	idle, err := net.Dial("tcp", raw.Addr().String())
	if err != nil {
		t.Fatal(err)
	}
	defer idle.Close()
	waitFor(t, "both connections to hold a slot", func() bool { return len(admitted.slots) == 2 })
	_, _ = c.Write([]byte("GET / HTTP/1.1\r\nHost: x\r\n\r\n"))
	var h net.Conn
	select {
	case h = <-hijacked:
	case <-time.After(5 * time.Second):
		t.Fatal("no hijacked connection")
	}
	if got := len(admitted.slots); got != 2 {
		t.Fatalf("slots in use while hijacked: %d, want 2", got)
	}
	_ = h.Close()
	_ = h.Close()
	if got := len(admitted.slots); got != 1 {
		t.Fatalf("slots in use after the hijacker closed twice: %d, want 1 (the idle one)", got)
	}
	set.mu.Lock()
	tracked := len(set.conns)
	set.mu.Unlock()
	if tracked != 1 {
		t.Fatalf("%d connections tracked, want only the idle one", tracked)
	}
}

// Peers that are not the owner, and lookups that fail, are closed before the
// server reads a TLS byte, are reported, and do not keep a slot.
func TestServiceRejectsUnauthorizedPeersAtAccept(t *testing.T) {
	cases := []struct {
		name   string
		whois  func(context.Context, string) (*apitype.WhoIsResponse, error)
		reason string
	}{
		{"tagged", func(context.Context, string) (*apitype.WhoIsResponse, error) { return whoFor(testLogin, "tag:x"), nil }, rejectTagged},
		{"other user", func(context.Context, string) (*apitype.WhoIsResponse, error) { return whoFor("other@example.com"), nil }, rejectOtherUser},
		{"whois failure", func(context.Context, string) (*apitype.WhoIsResponse, error) { return nil, io.ErrUnexpectedEOF }, rejectWhoisFailed},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			// Once the rejections are done the same peer becomes the owner.
			var owner atomic.Bool
			lc := &fakeLocal{whois: func(ctx context.Context, remote string) (*apitype.WhoIsResponse, error) {
				if owner.Load() {
					return whoFor(testLogin), nil
				}
				return tc.whois(ctx, remote)
			}}
			svc, addr, out := startAdmitService(t, lc)
			for i := 0; i < maxConns*2; i++ {
				c, err := net.Dial("tcp", addr)
				if err != nil {
					t.Fatal(err)
				}
				// The client says nothing: only an accept-time check closes it.
				if !closedByServer(c, 3*time.Second) {
					t.Fatalf("connection %d was not closed before any TLS byte", i)
				}
				_ = c.Close()
			}
			if f := waitFrame(t, out, "peer.rejected"); f["reason"] != tc.reason {
				t.Fatalf("reason %v, want %s", f["reason"], tc.reason)
			}
			if lc.calls() != 0 {
				t.Fatal("a certificate was requested for a rejected peer")
			}
			waitFor(t, "slots to be free", func() bool { return len(svc.conns.conns) == 0 })
			// Twice maxConns peers were rejected. If each kept its slot, the
			// owner would now be closed at once, locked out for good.
			owner.Store(true)
			c, err := net.Dial("tcp", addr)
			if err != nil {
				t.Fatal(err)
			}
			defer c.Close()
			if closedByServer(c, 300*time.Millisecond) {
				t.Fatal("the owner was refused after the rejected peers: their slots leaked")
			}
		})
	}
}

// The owner's devices still get through the accept-time check.
func TestServiceServesTheOwnerAfterAcceptCheck(t *testing.T) {
	cert, _ := selfSigned(t, testDNSName, time.Now().Add(time.Hour))
	_, addr, _ := startAdmitService(t, &fakeLocal{cert: cert})
	client := &http.Client{Transport: &http.Transport{
		TLSClientConfig:   &tls.Config{ServerName: testDNSName, InsecureSkipVerify: true},
		DisableKeepAlives: true,
	}}
	resp, err := client.Get("https://" + addr + "/")
	if err != nil {
		t.Fatal(err)
	}
	_ = resp.Body.Close()
	if resp.StatusCode != http.StatusBadGateway { // the gateway at :9 is down
		t.Fatalf("status %d, want 502", resp.StatusCode)
	}
}

// A lookup that the closing service cut short says nothing about the peer: the
// connection is closed and no peer.rejected is reported.
func TestServiceCloseDoesNotReportAHangingLookupAsRejected(t *testing.T) {
	started := make(chan struct{}, 1)
	lc := &fakeLocal{whois: func(ctx context.Context, _ string) (*apitype.WhoIsResponse, error) {
		started <- struct{}{}
		<-ctx.Done()
		return nil, ctx.Err()
	}}
	svc, addr, out := startAdmitService(t, lc)
	c, err := net.Dial("tcp", addr)
	if err != nil {
		t.Fatal(err)
	}
	defer c.Close()
	select {
	case <-started:
	case <-time.After(5 * time.Second):
		t.Fatal("the lookup never started")
	}
	svc.close()
	if !closedByServer(c, 3*time.Second) {
		t.Fatal("the connection was not closed")
	}
	// The admit goroutine reports (or not) right after its lookup returns.
	time.Sleep(300 * time.Millisecond)
	if got := out.ofType(t, "peer.rejected"); len(got) != 0 {
		t.Fatalf("a closing service reported a rejection: %v", got)
	}
}

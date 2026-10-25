package main

import (
	"bufio"
	"context"
	"errors"
	"io"
	"mime"
	"net"
	"net/http"
	"net/http/httptest"
	"net/url"
	"strings"
	"sync"
	"testing"
	"time"

	"tailscale.com/client/tailscale/apitype"
	"tailscale.com/tailcfg"
)

func TestAuthorize(t *testing.T) {
	tests := []struct {
		name       string
		who        *apitype.WhoIsResponse
		err        error
		wantReason string
		wantPeer   string
	}{
		{"whois error", nil, errors.New("no such peer"), rejectWhoisFailed, ""},
		{"whois error beats a response", ownerWho(), errors.New("boom"), rejectWhoisFailed, ""},
		{"no response", nil, nil, rejectWhoisFailed, ""},
		{"no node", &apitype.WhoIsResponse{UserProfile: &tailcfg.UserProfile{LoginName: testLogin}}, nil, rejectWhoisFailed, ""},
		{"tagged node", whoFor(testLogin, "tag:server"), nil, rejectTagged, "phone"},
		{"tagged node of another user", whoFor("other@example.com", "tag:server"), nil, rejectTagged, "phone"},
		{"other user", whoFor("other@example.com"), nil, rejectOtherUser, "phone"},
		{"login differs only by case", whoFor("Owner@example.com"), nil, rejectOtherUser, "phone"},
		{"no user profile", &apitype.WhoIsResponse{Node: &tailcfg.Node{ComputedName: "phone"}}, nil, rejectOtherUser, "phone"},
		{"owner", ownerWho(), nil, "", "phone"},
	}
	for _, tc := range tests {
		reason, peer := authorize(tc.who, tc.err, testLogin)
		if reason != tc.wantReason || peer != tc.wantPeer {
			t.Errorf("%s: got (%q, %q), want (%q, %q)", tc.name, reason, peer, tc.wantReason, tc.wantPeer)
		}
	}
}

func TestRewriteHeaders(t *testing.T) {
	h := http.Header{}
	for name, value := range map[string]string{
		"Tailscale-User-Login":   "forged@example.com",
		"tailscale-user-name":    "forged",
		"TAILSCALE-FUTURE":       "x",
		"Yzc-Proxy-Secret":       "forged",
		"yzc-peer-login":         "forged@example.com",
		"YZC-Other":              "x",
		"X-Forwarded-For":        "1.2.3.4",
		"x-forwarded-host":       "evil.example",
		"X-FORWARDED-PROTO":      "http",
		"X-Forwarded-Anything":   "x",
		"Forwarded":              "for=1.2.3.4",
		"FORWARDED":              "for=1.2.3.4",
		"Cookie":                 "session=abc",
		"Authorization":          "Bearer t",
		"Sec-WebSocket-Protocol": "yzc.v1",
		"X-Forward":              "kept",
		"Tailscale":              "kept",
		"Yzc":                    "kept",
	} {
		h[name] = []string{value} // raw keys: not canonicalised, as a hostile client could not send but a bug could
	}
	rewriteHeaders(h, testSecret, testLogin)

	for name := range h {
		switch strings.ToLower(name) {
		case "cookie", "authorization", "sec-websocket-protocol", "x-forward", "tailscale", "yzc", "yzc-proxy-secret", "yzc-peer-login":
		default:
			t.Errorf("header %q survived", name)
		}
	}
	if got := h.Values("Yzc-Proxy-Secret"); len(got) != 1 || got[0] != testSecret {
		t.Errorf("secret header = %v", got)
	}
	if got := h.Values("Yzc-Peer-Login"); len(got) != 1 || got[0] != testLogin {
		t.Errorf("login header = %v", got)
	}
	if h.Get("Cookie") != "session=abc" || h.Get("Authorization") != "Bearer t" {
		t.Errorf("unrelated headers must be kept: %v", h)
	}
}

func TestEncodeHeader(t *testing.T) {
	if got := encodeHeader("owner@example.com"); got != "owner@example.com" {
		t.Errorf("plain ASCII changed: %q", got)
	}
	for _, login := range []string{"ünï@example.com", "山田太郎@example.jp", "a\x7fb"} {
		got := encodeHeader(login)
		if !strings.HasPrefix(got, "=?utf-8?q?") || strings.ContainsAny(got, "\r\n") {
			t.Errorf("%q encoded as %q", login, got)
		}
		decoded, err := new(mime.WordDecoder).DecodeHeader(got)
		if err != nil || decoded != login {
			t.Errorf("%q round-trips to %q (%v)", login, decoded, err)
		}
	}
}

// upstream is a stand-in gateway that records what it receives and speaks a
// minimal upgrade echo.
type upstream struct {
	*httptest.Server
	mu       sync.Mutex
	requests []*http.Request
}

func newUpstream(t *testing.T) *upstream {
	u := &upstream{}
	u.Server = httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		u.mu.Lock()
		u.requests = append(u.requests, r.Clone(context.Background()))
		u.mu.Unlock()
		if strings.EqualFold(r.Header.Get("Upgrade"), "websocket") {
			conn, rw, err := w.(http.Hijacker).Hijack()
			if err != nil {
				return
			}
			defer conn.Close()
			_, _ = io.WriteString(rw, "HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n\r\n")
			_ = rw.Flush()
			_, _ = io.Copy(conn, rw)
			return
		}
		_, _ = io.WriteString(w, "gateway:"+r.URL.Path)
	}))
	t.Cleanup(u.Close)
	return u
}

func (u *upstream) seen() []*http.Request {
	u.mu.Lock()
	defer u.mu.Unlock()
	return append([]*http.Request(nil), u.requests...)
}

type rejection struct{ peer, reason string }

func proxyFixture(t *testing.T, lc *fakeLocal) (front *httptest.Server, gateway *upstream, rejected func() []rejection) {
	t.Helper()
	gateway = newUpstream(t)
	target, _ := url.Parse(gateway.URL)
	var mu sync.Mutex
	var got []rejection
	handler := newProxyHandler(lc, target, serveConfig{target: gateway.URL, proxySecret: testSecret, requiredLogin: testLogin},
		func(peer, reason string) {
			mu.Lock()
			defer mu.Unlock()
			got = append(got, rejection{peer, reason})
		}, discard)
	front = httptest.NewServer(handler)
	t.Cleanup(front.Close)
	return front, gateway, func() []rejection {
		mu.Lock()
		defer mu.Unlock()
		return append([]rejection(nil), got...)
	}
}

func TestProxyForwardsTheOwnerWithTrustedHeaders(t *testing.T) {
	front, gateway, rejected := proxyFixture(t, &fakeLocal{})
	req, _ := http.NewRequest("GET", front.URL+"/api/ping?x=1", nil)
	req.Host = testDNSName
	req.Header.Set("Tailscale-User-Login", "forged@example.com")
	req.Header.Set("Yzc-Proxy-Secret", "forged")
	req.Header.Add("Yzc-Peer-Login", "forged@example.com")
	req.Header.Add("Yzc-Peer-Login", "second@example.com")
	req.Header.Set("X-Forwarded-For", "9.9.9.9")
	req.Header.Set("Forwarded", "for=9.9.9.9")
	req.Header.Set("Connection", "Yzc-Proxy-Secret")
	req.Header.Set("Cookie", "k=v")
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	body, _ := io.ReadAll(resp.Body)
	resp.Body.Close()
	if resp.StatusCode != 200 || string(body) != "gateway:/api/ping" {
		t.Fatalf("response %d %q", resp.StatusCode, body)
	}
	seen := gateway.seen()
	if len(seen) != 1 {
		t.Fatalf("gateway saw %d requests", len(seen))
	}
	got := seen[0]
	if got.Host != testDNSName {
		t.Errorf("Host = %q, want the inbound %q", got.Host, testDNSName)
	}
	if got.URL.RawQuery != "x=1" || got.Header.Get("Cookie") != "k=v" {
		t.Errorf("request not forwarded as is: %v %v", got.URL, got.Header)
	}
	if v := got.Header.Values("Yzc-Proxy-Secret"); len(v) != 1 || v[0] != testSecret {
		t.Errorf("Yzc-Proxy-Secret = %v", v)
	}
	if v := got.Header.Values("Yzc-Peer-Login"); len(v) != 1 || v[0] != testLogin {
		t.Errorf("Yzc-Peer-Login = %v", v)
	}
	for _, name := range []string{"Tailscale-User-Login", "X-Forwarded-For", "X-Forwarded-Host", "X-Forwarded-Proto", "Forwarded"} {
		if v := got.Header.Values(name); len(v) != 0 {
			t.Errorf("%s reached the gateway: %v", name, v)
		}
	}
	if got := rejected(); len(got) != 0 {
		t.Errorf("owner was reported as rejected: %v", got)
	}
}

func TestProxyRejectsEveryoneElse(t *testing.T) {
	tests := []struct {
		name   string
		who    *apitype.WhoIsResponse
		err    error
		reason string
		peer   string
	}{
		{"whois failure", nil, errors.New("unknown peer"), rejectWhoisFailed, ""},
		{"tagged", whoFor(testLogin, "tag:ci"), nil, rejectTagged, "phone"},
		{"other user", whoFor("other@example.com"), nil, rejectOtherUser, "phone"},
	}
	for _, tc := range tests {
		lc := &fakeLocal{whois: func(context.Context, string) (*apitype.WhoIsResponse, error) { return tc.who, tc.err }}
		front, gateway, rejected := proxyFixture(t, lc)
		resp, err := http.Get(front.URL + "/")
		if err != nil {
			t.Fatal(err)
		}
		body, _ := io.ReadAll(resp.Body)
		resp.Body.Close()
		if resp.StatusCode != 403 || string(body) != "Forbidden" {
			t.Errorf("%s: %d %q", tc.name, resp.StatusCode, body)
		}
		if len(gateway.seen()) != 0 {
			t.Errorf("%s: the gateway was reached", tc.name)
		}
		if got := rejected(); len(got) != 1 || got[0] != (rejection{tc.peer, tc.reason}) {
			t.Errorf("%s: rejections %v", tc.name, got)
		}
	}
}

func TestProxyPassesWebSocketUpgrades(t *testing.T) {
	front, gateway, _ := proxyFixture(t, &fakeLocal{})
	conn, err := net.Dial("tcp", front.Listener.Addr().String())
	if err != nil {
		t.Fatal(err)
	}
	defer conn.Close()
	_ = conn.SetDeadline(time.Now().Add(5 * time.Second))
	request := "GET /ws HTTP/1.1\r\nHost: " + testDNSName + "\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n" +
		"Sec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nYzc-Proxy-Secret: forged\r\n\r\n"
	if _, err := io.WriteString(conn, request); err != nil {
		t.Fatal(err)
	}
	reader := bufio.NewReader(conn)
	resp, err := http.ReadResponse(reader, nil)
	if err != nil || resp.StatusCode != http.StatusSwitchingProtocols {
		t.Fatalf("upgrade: %v %v", resp, err)
	}
	if _, err := io.WriteString(conn, "ping"); err != nil {
		t.Fatal(err)
	}
	echo := make([]byte, 4)
	if _, err := io.ReadFull(reader, echo); err != nil || string(echo) != "ping" {
		t.Fatalf("echo through the upgraded connection: %q %v", echo, err)
	}
	seen := gateway.seen()
	if len(seen) != 1 || seen[0].Header.Get("Yzc-Proxy-Secret") != testSecret || seen[0].Header.Get("Yzc-Peer-Login") != testLogin {
		t.Errorf("the upgrade must carry the trusted headers: %v", seen)
	}
}

func TestRejectLimiter(t *testing.T) {
	now := time.Unix(1_000, 0)
	limiter := &rejectLimiter{now: func() time.Time { return now }, last: map[rejectKey]time.Time{}}

	if !limiter.allow("phone", rejectTagged) {
		t.Fatal("first rejection must be reported")
	}
	if limiter.allow("phone", rejectTagged) {
		t.Fatal("repeat within the window must be suppressed")
	}
	if !limiter.allow("phone", rejectOtherUser) || !limiter.allow("laptop", rejectTagged) || !limiter.allow("", rejectWhoisFailed) {
		t.Fatal("another reason or node has its own window")
	}
	now = now.Add(59 * time.Second)
	if limiter.allow("phone", rejectTagged) {
		t.Fatal("59 s later is still inside the window")
	}
	now = now.Add(time.Second)
	if !limiter.allow("phone", rejectTagged) {
		t.Fatal("60 s later is a new window")
	}
}

func TestRejectLimiterIsBounded(t *testing.T) {
	now := time.Unix(1_000, 0)
	limiter := &rejectLimiter{now: func() time.Time { return now }, last: map[rejectKey]time.Time{}}
	for i := 0; i < maxRejectKeys; i++ {
		if !limiter.allow(strings.Repeat("n", i+1), rejectTagged) {
			t.Fatalf("key %d was dropped before the bound", i)
		}
	}
	if limiter.allow("one-more", rejectTagged) {
		t.Fatal("a full table of live keys must drop new ones")
	}
	now = now.Add(rejectWindow)
	if !limiter.allow("one-more", rejectTagged) || len(limiter.last) != 1 {
		t.Fatalf("expired keys must be evicted: %d left", len(limiter.last))
	}
}

func TestRejectReporterEmitsOncePerWindow(t *testing.T) {
	emit, out := newTestEmitter()
	now := time.Unix(1_000, 0)
	report := rejectReporter(emit, func() time.Time { return now })
	for i := 0; i < 5; i++ {
		report("phone", rejectTagged)
	}
	report("", rejectWhoisFailed)
	frames := out.ofType(t, "peer.rejected")
	if len(frames) != 2 || frames[0]["reason"] != rejectTagged || frames[0]["node"] != "phone" ||
		frames[1]["reason"] != rejectWhoisFailed || frames[1]["node"] != "" {
		t.Fatalf("peer.rejected frames: %v", frames)
	}
	now = now.Add(rejectWindow)
	report("phone", rejectTagged)
	if got := len(out.ofType(t, "peer.rejected")); got != 3 {
		t.Fatalf("a new window must report again: %d frames", got)
	}
}

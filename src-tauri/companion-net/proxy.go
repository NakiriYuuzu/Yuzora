package main

import (
	"context"
	"log"
	"mime"
	"net/http"
	"net/http/httputil"
	"net/url"
	"strings"
	"sync"
	"time"

	"tailscale.com/client/tailscale/apitype"
)

const (
	rejectWhoisFailed = "whois-failed"
	rejectTagged      = "tagged"
	rejectOtherUser   = "other-user"

	rejectWindow  = 60 * time.Second
	maxRejectKeys = 1024
	whoisTimeout  = 5 * time.Second
)

// authorize decides whether a WhoIs answer may reach the gateway. It returns
// the reject reason, empty when the peer is the owner, and the peer's node name.
func authorize(who *apitype.WhoIsResponse, err error, requiredLogin string) (reason, peer string) {
	if err != nil || who == nil || who.Node == nil {
		return rejectWhoisFailed, ""
	}
	peer = who.Node.ComputedName
	if who.Node.IsTagged() {
		return rejectTagged, peer
	}
	if who.UserProfile == nil || who.UserProfile.LoginName != requiredLogin {
		return rejectOtherUser, peer
	}
	return "", peer
}

// rewriteHeaders drops every header a client could use to claim an identity or
// a forwarding chain, then sets the two the gateway trusts.
func rewriteHeaders(h http.Header, proxySecret, login string) {
	for name := range h {
		lower := strings.ToLower(name)
		if strings.HasPrefix(lower, "tailscale-") || strings.HasPrefix(lower, "yzc-") ||
			strings.HasPrefix(lower, "x-forwarded-") || lower == "forwarded" {
			delete(h, name)
		}
	}
	h.Set("Yzc-Proxy-Secret", proxySecret)
	h.Set("Yzc-Peer-Login", encodeHeader(login))
}

// encodeHeader applies RFC 2047 Q-encoding when the value is not plain ASCII,
// as `tailscale serve` does for Tailscale-User-Name.
func encodeHeader(value string) string {
	for i := 0; i < len(value); i++ {
		if value[i] < 0x20 || value[i] > 0x7e {
			return mime.QEncoding.Encode("utf-8", value)
		}
	}
	return value
}

// newProxyHandler admits only the owner's devices and proxies them to the
// gateway. Everything else gets a bare 403.
func newProxyHandler(lc localAPI, target *url.URL, cfg serveConfig, reject func(peer, reason string), diag *log.Logger) http.Handler {
	proxy := &httputil.ReverseProxy{
		Rewrite: func(pr *httputil.ProxyRequest) {
			pr.SetURL(target)
			pr.Out.Host = pr.In.Host
			rewriteHeaders(pr.Out.Header, cfg.proxySecret, cfg.requiredLogin)
		},
		ErrorLog: diag,
	}
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		ctx, cancel := context.WithTimeout(r.Context(), whoisTimeout)
		who, err := lc.WhoIs(ctx, r.RemoteAddr)
		cancel()
		if reason, peer := authorize(who, err, cfg.requiredLogin); reason != "" {
			reject(peer, reason)
			w.Header().Set("Content-Type", "text/plain; charset=utf-8")
			w.WriteHeader(http.StatusForbidden)
			_, _ = w.Write([]byte("Forbidden"))
			return
		}
		proxy.ServeHTTP(w, r)
	})
}

type rejectKey struct{ peer, reason string }

// rejectLimiter lets one peer.rejected through per (node, reason) per window.
type rejectLimiter struct {
	mu   sync.Mutex
	now  func() time.Time
	last map[rejectKey]time.Time
}

func (l *rejectLimiter) allow(peer, reason string) bool {
	now := l.now()
	l.mu.Lock()
	defer l.mu.Unlock()
	key := rejectKey{peer, reason}
	if at, ok := l.last[key]; ok && now.Sub(at) < rejectWindow {
		return false
	}
	if len(l.last) >= maxRejectKeys {
		for old, at := range l.last {
			if now.Sub(at) >= rejectWindow {
				delete(l.last, old)
			}
		}
		if len(l.last) >= maxRejectKeys {
			return false
		}
	}
	l.last[key] = now
	return true
}

// rejectReporter emits peer.rejected for rejected requests, rate limited.
func rejectReporter(emit *emitter, now func() time.Time) func(peer, reason string) {
	limiter := &rejectLimiter{now: now, last: map[rejectKey]time.Time{}}
	return func(peer, reason string) {
		if limiter.allow(peer, reason) {
			emit.send("peer.rejected", map[string]any{"reason": reason, "node": peer})
		}
	}
}

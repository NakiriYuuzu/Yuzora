package main

import (
	"crypto/tls"
	"log"
	"net"
	"net/http"
	"net/url"
	"strings"
	"sync"
	"testing"
	"time"
)

// testClock is a clock tests move by hand; the server goroutines read it too.
type testClock struct {
	mu  sync.Mutex
	now time.Time
}

func newTestClock() *testClock { return &testClock{now: time.Unix(1_000, 0)} }

func (c *testClock) read() time.Time {
	c.mu.Lock()
	defer c.mu.Unlock()
	return c.now
}

func (c *testClock) advance(d time.Duration) {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.now = c.now.Add(d)
}

// logLines returns the non-empty lines written so far.
func (b *syncBuffer) logLines() []string {
	b.mu.Lock()
	defer b.mu.Unlock()
	var out []string
	for _, line := range strings.Split(b.buf.String(), "\n") {
		if line != "" {
			out = append(out, line)
		}
	}
	return out
}

func (b *syncBuffer) countContaining(s string) int {
	n := 0
	for _, line := range b.logLines() {
		if strings.Contains(line, s) {
			n++
		}
	}
	return n
}

func limitedLogFixture(t *testing.T) (*limitedLog, *log.Logger, *syncBuffer, *testClock) {
	t.Helper()
	var lines syncBuffer
	clock := newTestClock()
	l := newLimitedLog(log.New(&lines, "yuzora-companion-net: ", 0), clock.read)
	t.Cleanup(l.close)
	return l, log.New(l, "", 0), &lines, clock
}

func TestLimitedLogPassesTheFirstLinesThroughUnchanged(t *testing.T) {
	_, logger, lines, _ := limitedLogFixture(t)
	logger.Printf("http: proxy error: %v", "dial tcp: refused")
	logger.Print("second")
	got := lines.logLines()
	if len(got) != 2 || got[0] != "yuzora-companion-net: http: proxy error: dial tcp: refused" || got[1] != "yuzora-companion-net: second" {
		t.Fatalf("lines under the limit must pass through with the sidecar's prefix: %q", got)
	}
}

func TestLimitedLogCapsAWindowAndSummarisesWhenItRolls(t *testing.T) {
	l, logger, lines, clock := limitedLogFixture(t)
	for i := 0; i < 2000; i++ {
		logger.Printf("flood %d", i)
	}
	if got := lines.logLines(); len(got) != connLogLines {
		t.Fatalf("a window lets %d lines through, got %d", connLogLines, len(got))
	}
	clock.advance(connLogWindow - time.Second)
	logger.Print("still the same window")
	if got := lines.logLines(); len(got) != connLogLines || lines.countContaining("suppressed") != 0 {
		t.Fatalf("nothing may pass before the window ends: %d lines", len(got))
	}

	clock.advance(time.Second)
	logger.Print("next window")
	got := lines.logLines()
	if len(got) != connLogLines+2 {
		t.Fatalf("want the summary and the new line after %d: %q", connLogLines, got[connLogLines:])
	}
	// 1990 of the flood plus the one line of the "same window" probe.
	if got[connLogLines] != "yuzora-companion-net: … 1991 similar lines suppressed" || got[connLogLines+1] != "yuzora-companion-net: next window" {
		t.Fatalf("summary and next line: %q", got[connLogLines:])
	}

	// The new window has its own allowance.
	for i := 0; i < 50; i++ {
		logger.Print("again")
	}
	l.mu.Lock()
	passed, dropped := l.passed, l.dropped
	l.mu.Unlock()
	if passed != connLogLines || dropped != 51-connLogLines {
		t.Fatalf("second window: passed=%d dropped=%d", passed, dropped)
	}
}

func TestLimitedLogQuietWindowsSayNothing(t *testing.T) {
	_, logger, lines, clock := limitedLogFixture(t)
	for window := 0; window < 3; window++ {
		logger.Print("one line")
		clock.advance(connLogWindow)
	}
	logger.Print("one line")
	if lines.countContaining("suppressed") != 0 || len(lines.logLines()) != 4 {
		t.Fatalf("no line was dropped, so no summary: %q", lines.logLines())
	}
}

// A peer that floods and stops sends no further line to trigger the summary.
func TestLimitedLogSummarisesWithoutAFurtherLine(t *testing.T) {
	var lines syncBuffer
	l := newLimitedLog(log.New(&lines, "", 0), time.Now)
	l.window = 100 * time.Millisecond
	t.Cleanup(l.close)
	logger := log.New(l, "", 0)
	for i := 0; i < connLogLines+7; i++ {
		logger.Print("flood")
	}
	waitFor(t, "the summary", func() bool { return lines.countContaining("… 7 similar lines suppressed") == 1 })
	time.Sleep(250 * time.Millisecond)
	if lines.countContaining("suppressed") != 1 {
		t.Fatalf("the summary is said once: %q", lines.logLines())
	}
}

func TestLimitedLogCloseSaysWhatIsStillCounted(t *testing.T) {
	l, logger, lines, _ := limitedLogFixture(t)
	for i := 0; i < connLogLines+3; i++ {
		logger.Print("flood")
	}
	l.close()
	l.close()
	if lines.countContaining("… 3 similar lines suppressed") != 1 {
		t.Fatalf("close summary: %q", lines.logLines())
	}
}

// Failed TLS handshakes stay within the limit and are summarised once, however
// many there are. (Peers that are not the owner never get that far: they are
// closed at accept, see admitListener. This checks the limiter itself, fed by
// handshakes that fail for any other reason.)
func TestServiceFloodOfFailedHandshakesIsRateLimited(t *testing.T) {
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	target, _ := url.Parse("http://127.0.0.1:9")
	var stderr syncBuffer
	diag := log.New(&stderr, "yuzora-companion-net: ", log.LstdFlags)
	emit := &emitter{w: &syncBuffer{}, diag: diag}
	clock := newTestClock()
	svc := startService(ln, &fakeLocal{}, func() string { return testDNSName }, target,
		serveConfig{target: target.String(), proxySecret: testSecret, requiredLogin: testLogin}, emit, diag, clock.read)
	defer svc.close()

	const flood = 2000
	count := func() int {
		svc.logs.mu.Lock()
		defer svc.logs.mu.Unlock()
		return svc.logs.passed + svc.logs.dropped
	}
	for i := 0; i < flood; i++ {
		c, err := net.Dial("tcp", ln.Addr().String())
		if err != nil {
			t.Fatal(err)
		}
		_, _ = c.Write([]byte("x"))
		_ = c.Close()
		// Stay below the connection cap: a connection closed for want of a
		// slot never reaches the handshake this test counts.
		if (i+1)%(maxConns/2) == 0 {
			waitFor(t, "the handshakes so far", func() bool { return count() == i+1 })
		}
	}
	waitFor(t, "the server to see every connection", func() bool { return count() == flood })
	if got := stderr.countContaining("TLS handshake error"); got != connLogLines {
		t.Fatalf("%d handshake failures wrote %d stderr lines, want %d", flood, got, connLogLines)
	}

	clock.advance(connLogWindow)
	c, err := net.Dial("tcp", ln.Addr().String())
	if err != nil {
		t.Fatal(err)
	}
	_, _ = c.Write([]byte("x"))
	_ = c.Close()
	waitFor(t, "the summary", func() bool { return stderr.countContaining("… 1990 similar lines suppressed") == 1 })
}

// Closing the service says what its last window still holds.
func TestServiceCloseSummarisesTheLastWindow(t *testing.T) {
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	target, _ := url.Parse("http://127.0.0.1:9")
	var stderr syncBuffer
	diag := log.New(&stderr, "", 0)
	emit := &emitter{w: &syncBuffer{}, diag: diag}
	svc := startService(ln, &fakeLocal{}, func() string { return testDNSName }, target,
		serveConfig{target: target.String(), proxySecret: testSecret, requiredLogin: testLogin}, emit, diag, newTestClock().read)

	const failures = connLogLines + 5
	for i := 0; i < failures; i++ {
		c, err := net.Dial("tcp", ln.Addr().String())
		if err != nil {
			t.Fatal(err)
		}
		_, _ = c.Write([]byte("x"))
		_ = c.Close()
	}
	waitFor(t, "the server to see every connection", func() bool {
		svc.logs.mu.Lock()
		defer svc.logs.mu.Unlock()
		return svc.logs.passed+svc.logs.dropped == failures
	})
	svc.close()
	if stderr.countContaining("… 5 similar lines suppressed") != 1 {
		t.Fatalf("close must report the dropped lines: %q", stderr.logLines())
	}
}

// The reverse proxy logs once per failed request of the owner's devices; it
// shares the limit with the server.
func TestServiceProxyErrorsShareTheLimit(t *testing.T) {
	down, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	gatewayAddr := down.Addr().String()
	_ = down.Close() // nothing listens there: every proxied request fails
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	target, _ := url.Parse("http://" + gatewayAddr)
	var stderr syncBuffer
	diag := log.New(&stderr, "", 0)
	emit := &emitter{w: &syncBuffer{}, diag: diag}
	cert, _ := selfSigned(t, testDNSName, time.Now().Add(time.Hour))
	svc := startService(ln, &fakeLocal{cert: cert}, func() string { return testDNSName }, target,
		serveConfig{target: target.String(), proxySecret: testSecret, requiredLogin: testLogin}, emit, diag, newTestClock().read)

	client := &http.Client{Transport: &http.Transport{
		TLSClientConfig:   &tls.Config{ServerName: testDNSName, InsecureSkipVerify: true},
		DisableKeepAlives: true,
	}}
	const requests = connLogLines + 20
	for i := 0; i < requests; i++ {
		resp, err := client.Get("https://" + ln.Addr().String() + "/")
		if err != nil {
			t.Fatal(err)
		}
		_ = resp.Body.Close()
		if resp.StatusCode != http.StatusBadGateway {
			t.Fatalf("status %d, want 502", resp.StatusCode)
		}
	}
	if got := stderr.countContaining("proxy error"); got != connLogLines {
		t.Fatalf("%d failed proxy requests wrote %d lines, want %d", requests, got, connLogLines)
	}
	svc.close()
	if stderr.countContaining("… 20 similar lines suppressed") != 1 {
		t.Fatalf("close must report the dropped lines: %q", stderr.logLines())
	}
}

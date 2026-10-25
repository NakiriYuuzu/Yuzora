package main

import (
	"log"
	"strings"
	"sync"
	"time"
)

const (
	// connLogLines is how many per-connection lines reach stderr per window.
	connLogLines  = 10
	connLogWindow = time.Minute
)

// limitedLog is the writer behind the loggers that fire once per peer action:
// the HTTP server's ErrorLog (every failed TLS handshake) and the reverse
// proxy's. Any tailnet peer that can reach the port makes those lines at will,
// and stderr ends up in the desktop log, so per window only the first max
// lines pass to out; the rest are counted and summarised once the window ends.
type limitedLog struct {
	out    *log.Logger // adds the prefix and the timestamp
	now    func() time.Time
	max    int
	window time.Duration

	mu      sync.Mutex
	start   time.Time // when the current window opened; zero while none is open
	passed  int
	dropped int
	timer   *time.Timer // says the summary when no further line arrives
	closed  bool
}

func newLimitedLog(out *log.Logger, now func() time.Time) *limitedLog {
	return &limitedLog{out: out, now: now, max: connLogLines, window: connLogWindow}
}

func (l *limitedLog) Write(p []byte) (int, error) {
	l.mu.Lock()
	defer l.mu.Unlock()
	now := l.now()
	l.roll(now)
	if l.start.IsZero() {
		l.start = now
	}
	if l.passed < l.max {
		l.passed++
		l.out.Print(strings.TrimRight(string(p), "\n"))
		return len(p), nil
	}
	if l.dropped == 0 && !l.closed {
		if l.timer != nil {
			l.timer.Stop()
		}
		l.timer = time.AfterFunc(l.start.Add(l.window).Sub(now), l.expire)
	}
	l.dropped++
	return len(p), nil
}

// roll closes the current window once it has run its course. The caller holds l.mu.
func (l *limitedLog) roll(now time.Time) {
	if l.start.IsZero() || now.Sub(l.start) < l.window {
		return
	}
	l.summarise()
	l.start = time.Time{}
}

// summarise reports what the window dropped and starts the count over. The
// caller holds l.mu.
func (l *limitedLog) summarise() {
	if l.dropped > 0 {
		l.out.Printf("… %d similar lines suppressed", l.dropped)
	}
	l.passed, l.dropped = 0, 0
}

// expire is the timer: the summary must not wait for the next line, which a
// peer that stopped flooding never sends.
func (l *limitedLog) expire() {
	l.mu.Lock()
	defer l.mu.Unlock()
	l.roll(l.now())
}

// close says what is still counted and stops the timer.
func (l *limitedLog) close() {
	l.mu.Lock()
	defer l.mu.Unlock()
	l.closed = true
	if l.timer != nil {
		l.timer.Stop()
	}
	l.summarise()
	l.start = time.Time{}
}

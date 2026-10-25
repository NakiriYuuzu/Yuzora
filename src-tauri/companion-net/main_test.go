package main

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"io"
	"log"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"tailscale.com/envknob"
	"tailscale.com/ipn/ipnstate"
)

const testVersion = "0.0.19-test"

// runFixture drives run() over pipes with a fake tailnet.
type runFixture struct {
	in      *io.PipeWriter
	out     *syncBuffer
	ts      *fakeTailnet
	local   *fakeLocal
	result  chan int
	started chan struct{}
	dir     string
	host    string
	// stateDir is the directory init announces.
	stateDir string
}

// initLine is an init frame for dir. It is JSON-encoded so a Windows path
// survives, and callers pass t.TempDir() paths: validateInit wants an absolute
// path, which on Windows needs a drive letter, so a literal "/tmp/..." is relative there.
func initLine(t *testing.T, dir, version string) string {
	t.Helper()
	line, err := json.Marshal(map[string]any{"v": 1, "type": "init", "stateDir": dir, "hostname": "yuzora", "version": version})
	if err != nil {
		t.Fatal(err)
	}
	return string(line)
}

func startRun(t *testing.T, tweak func(*runConfig, *fakeTailnet)) *runFixture {
	t.Helper()
	previous := version
	version = testVersion
	t.Cleanup(func() { version = previous })

	reader, writer := io.Pipe()
	t.Cleanup(func() { writer.Close() })
	f := &runFixture{in: writer, out: &syncBuffer{}, result: make(chan int, 1), stateDir: filepath.Join(t.TempDir(), "net")}
	f.local = &fakeLocal{}
	f.local.setStatus(&ipnstate.Status{BackendState: "Starting"})
	f.ts = newFakeTailnet(f.local)
	cfg := runConfig{
		in:          reader,
		out:         f.out,
		diag:        discard,
		initTimeout: 5 * time.Second,
		closeGrace:  200 * time.Millisecond,
	}
	cfg.newTailnet = func(dir, hostname string) (tailnet, error) {
		f.dir, f.host = dir, hostname
		return f.ts, nil
	}
	if tweak != nil {
		tweak(&cfg, f.ts)
	}
	ctx, cancel := context.WithCancel(context.Background())
	t.Cleanup(cancel)
	go func() { f.result <- run(ctx, cfg) }()
	return f
}

func (f *runFixture) send(t *testing.T, line string) {
	t.Helper()
	if _, err := io.WriteString(f.in, line+"\n"); err != nil {
		t.Fatalf("writing %q: %v", line, err)
	}
}

func (f *runFixture) init(t *testing.T) {
	t.Helper()
	f.send(t, initLine(t, f.stateDir, testVersion))
	waitFrame(t, f.out, "started")
}

func (f *runFixture) exitCode(t *testing.T) int {
	t.Helper()
	select {
	case code := <-f.result:
		return code
	case <-time.After(5 * time.Second):
		t.Fatal("run did not return")
		return -1
	}
}

func errorCodes(t *testing.T, out *syncBuffer) []string {
	var codes []string
	for _, frame := range out.ofType(t, "error") {
		codes = append(codes, frame["code"].(string))
	}
	return codes
}

func TestRunHandshakeFailuresExitWithTwo(t *testing.T) {
	long := strings.Repeat("x", maxFrameBytes+1)
	dir := filepath.Join(t.TempDir(), "net")
	tests := []struct {
		name string
		line string
		code string
	}{
		{"version mismatch", initLine(t, dir, "9.9.9"), "net-version-mismatch"},
		{"relative stateDir", initLine(t, "rel", testVersion), "net-init-invalid"},
		{"missing stateDir", `{"v":1,"type":"init","hostname":"yuzora","version":"` + testVersion + `"}`, "net-init-invalid"},
		{"not init first", `{"v":1,"type":"serve"}`, "net-init-invalid"},
		{"not json", `hello`, "net-init-invalid"},
		{"oversized", long, "net-init-invalid"},
	}
	for _, tc := range tests {
		f := startRun(t, nil)
		go func() { _, _ = io.WriteString(f.in, tc.line+"\n") }()
		if code := f.exitCode(t); code != exitProtocol {
			t.Errorf("%s: exit %d, want 2", tc.name, code)
		}
		if got := errorCodes(t, f.out); len(got) != 1 || got[0] != tc.code {
			t.Errorf("%s: error codes %v, want %s", tc.name, got, tc.code)
		}
		if len(f.out.ofType(t, "started")) != 0 || f.host != "" {
			t.Errorf("%s: the node must not start", tc.name)
		}
	}
}

func TestRunWithoutInitExitsWithTwo(t *testing.T) {
	f := startRun(t, func(cfg *runConfig, _ *fakeTailnet) { cfg.initTimeout = 50 * time.Millisecond })
	if code := f.exitCode(t); code != exitProtocol {
		t.Fatalf("exit %d, want 2", code)
	}
	if got := errorCodes(t, f.out); len(got) != 1 || got[0] != "net-init-invalid" {
		t.Fatalf("error codes %v", got)
	}
}

func TestRunStdinClosedBeforeInit(t *testing.T) {
	f := startRun(t, nil)
	f.in.Close()
	if code := f.exitCode(t); code != exitProtocol {
		t.Fatalf("exit %d, want 2", code)
	}
}

func TestRunShutdown(t *testing.T) {
	f := startRun(t, nil)
	f.init(t)
	if f.dir != f.stateDir || f.host != "yuzora" {
		t.Errorf("node created for %q %q", f.dir, f.host)
	}
	if started := f.out.ofType(t, "started"); started[0]["version"] != testVersion {
		t.Errorf("started: %v", started)
	}
	waitFrame(t, f.out, "state") // the poll loop is alive
	f.send(t, `{"v":1,"type":"shutdown"}`)
	if code := f.exitCode(t); code != exitOK {
		t.Fatalf("exit %d, want 0", code)
	}
	select {
	case <-f.ts.closed:
	default:
		t.Fatal("the node was not closed")
	}
}

func TestRunStdinEOFMeansShutdown(t *testing.T) {
	f := startRun(t, nil)
	f.init(t)
	f.in.Close()
	if code := f.exitCode(t); code != exitOK {
		t.Fatalf("exit %d, want 0", code)
	}
	<-f.ts.closed
}

func TestRunStartFailure(t *testing.T) {
	f := startRun(t, func(_ *runConfig, ts *fakeTailnet) { ts.startErr = errors.New("cannot bind") })
	f.send(t, initLine(t, f.stateDir, testVersion))
	if code := f.exitCode(t); code != exitFailure {
		t.Fatalf("exit %d, want 1", code)
	}
	if got := errorCodes(t, f.out); len(got) != 1 || got[0] != "net-start-failed" {
		t.Fatalf("error codes %v", got)
	}
}

func TestRunCreatingTheNodeFails(t *testing.T) {
	f := startRun(t, func(cfg *runConfig, _ *fakeTailnet) {
		cfg.newTailnet = func(string, string) (tailnet, error) { return nil, errors.New("state dir not writable") }
	})
	f.send(t, initLine(t, f.stateDir, testVersion))
	if code := f.exitCode(t); code != exitFailure {
		t.Fatalf("exit %d, want 1", code)
	}
	if got := errorCodes(t, f.out); len(got) != 1 || got[0] != "net-start-failed" {
		t.Fatalf("error codes %v", got)
	}
}

func TestRunCommandsAfterInit(t *testing.T) {
	f := startRun(t, nil)
	f.init(t)

	f.send(t, `{"v":1,"type":"serve","target":"http://127.0.0.1:47110","proxySecret":"`+testSecret+`","requiredLogin":"`+testLogin+`"}`)
	waitFor(t, "net-not-ready", func() bool { return len(errorCodes(t, f.out)) == 1 })
	f.send(t, `{"v":1,"type":"serve","target":"http://localhost:1","proxySecret":"`+testSecret+`","requiredLogin":"`+testLogin+`"}`)
	waitFor(t, "net-serve-invalid", func() bool { return len(errorCodes(t, f.out)) == 2 })
	if got := errorCodes(t, f.out); got[0] != "net-not-ready" || got[1] != "net-serve-invalid" {
		t.Fatalf("error codes %v", got)
	}

	// Garbage and unknown frames are ignored; the process keeps serving commands.
	f.send(t, `garbage`)
	f.send(t, `{"v":1,"type":"mystery"}`)
	f.send(t, initLine(t, f.stateDir+"-other", testVersion))
	f.send(t, `{"v":1,"type":"unserve"}`)
	waitFrame(t, f.out, "unserved")

	f.local.logout = func(context.Context) error { return errors.New("offline") }
	f.send(t, `{"v":1,"type":"logout"}`)
	waitFor(t, "net-logout-failed", func() bool { return len(errorCodes(t, f.out)) == 3 })
	if len(f.out.ofType(t, "logged-out")) != 0 {
		t.Fatal("logged-out after a failed logout")
	}

	f.local.logout = nil
	f.send(t, `{"v":1,"type":"logout"}`)
	waitFrame(t, f.out, "logged-out")
	if code := f.exitCode(t); code != exitOK {
		t.Fatalf("exit %d after logout, want 0", code)
	}
	<-f.ts.closed
	if types := f.out.types(t); types[len(types)-1] != "logged-out" {
		t.Errorf("logged-out must be the last frame: %v", types)
	}
}

func TestRunOversizedFrameAfterInitStops(t *testing.T) {
	f := startRun(t, nil)
	f.init(t)
	go func() { _, _ = io.WriteString(f.in, strings.Repeat("x", maxFrameBytes+1)+"\n") }()
	if code := f.exitCode(t); code != exitProtocol {
		t.Fatalf("exit %d, want 2", code)
	}
	<-f.ts.closed
}

func TestRunStopsOnSignal(t *testing.T) {
	previous := version
	version = testVersion
	defer func() { version = previous }()

	reader, writer := io.Pipe()
	defer writer.Close()
	out := &syncBuffer{}
	local := &fakeLocal{}
	local.setStatus(&ipnstate.Status{BackendState: "Starting"})
	ts := newFakeTailnet(local)
	ctx, cancel := context.WithCancel(context.Background())
	result := make(chan int, 1)
	go func() {
		result <- run(ctx, runConfig{in: reader, out: out, diag: discard, initTimeout: 5 * time.Second, closeGrace: 200 * time.Millisecond,
			newTailnet: func(string, string) (tailnet, error) { return ts, nil }})
	}()
	_, _ = io.WriteString(writer, initLine(t, filepath.Join(t.TempDir(), "net"), testVersion)+"\n")
	waitFrame(t, out, "started")
	cancel()
	select {
	case code := <-result:
		if code != exitOK {
			t.Fatalf("exit %d, want 0", code)
		}
	case <-time.After(5 * time.Second):
		t.Fatal("run ignored the signal")
	}
}

func TestCloseWithinGivesUpOnAHangingNode(t *testing.T) {
	hang := &hangingTailnet{release: make(chan struct{})}
	defer close(hang.release)
	begin := time.Now()
	closeWithin(hang, 100*time.Millisecond)
	if elapsed := time.Since(begin); elapsed < 90*time.Millisecond || elapsed > time.Second {
		t.Fatalf("closeWithin took %v", elapsed)
	}
}

type hangingTailnet struct {
	fakeTailnet
	release chan struct{}
}

func (h *hangingTailnet) Close() error { <-h.release; return nil }

func TestSetupDisablesLogUpload(t *testing.T) {
	// t.Setenv restores the variable; envknob reads the environment on each call.
	t.Setenv("TS_NO_LOGS_NO_SUPPORT", "")
	previous := log.Writer()
	t.Cleanup(func() { log.SetOutput(previous) })
	if envknob.NoLogsNoSupport() {
		t.Fatal("the knob is already on, so the test proves nothing")
	}
	setupProcess()
	if !envknob.NoLogsNoSupport() {
		t.Fatal("tsnet would upload the node's logs to log.tailscale.com")
	}
}

// A dependency that logs through the std logger (tshttpproxy prints the proxy URL
// with its credentials) must not reach stderr, which the desktop writes to its log.
func TestStdLogNeverReachesStderr(t *testing.T) {
	if os.Getenv("YZC_STDLOG_HELPER") == "1" {
		setupProcess()
		log.Printf("tshttpproxy: using proxy %q for URL: %q", "http://alice:s3cretPW@127.0.0.1:3128", "https://controlplane.tailscale.com/")
		return
	}
	cmd := exec.Command(os.Args[0], "-test.run=^TestStdLogNeverReachesStderr$")
	cmd.Env = append(os.Environ(), "YZC_STDLOG_HELPER=1")
	var stderr bytes.Buffer
	cmd.Stderr = &stderr
	if err := cmd.Run(); err != nil {
		t.Fatalf("helper process: %v\n%s", err, stderr.String())
	}
	if stderr.Len() != 0 {
		t.Fatalf("the std logger reached stderr: %q", stderr.String())
	}
}

// runnableMalformedFrames are the malformed lines that fit one frame; the
// oversize ones are a fatal read error covered by TestRunOversizedFrameAfterInitStops.
func runnableMalformedFrames() map[string]string {
	lines := map[string]string{
		"nesting 30000 deep": `{"v":1,"type":"serve","x":` + strings.Repeat("[", 30000) + strings.Repeat("]", 30000) + `}`,
	}
	for name, line := range malformedFrames() {
		if line != "" && len(line) <= maxFrameBytes {
			lines[name] = line
		}
	}
	return lines
}

func TestRunMalformedHandshakeFramesExitWithTwo(t *testing.T) {
	for name, line := range runnableMalformedFrames() {
		f := startRun(t, nil)
		go func() { _, _ = io.WriteString(f.in, line+"\n") }()
		if code := f.exitCode(t); code != exitProtocol {
			t.Errorf("%s: exit %d, want 2", name, code)
		}
		if got := errorCodes(t, f.out); len(got) != 1 || got[0] != "net-init-invalid" {
			t.Errorf("%s: error codes %v, want net-init-invalid", name, got)
		}
		if len(f.out.ofType(t, "started")) != 0 || f.host != "" {
			t.Errorf("%s: the node must not start", name)
		}
	}
}

func TestRunMalformedFramesAfterInitAreIgnored(t *testing.T) {
	f := startRun(t, nil)
	f.init(t)
	// Written from a goroutine: if a frame wrongly stopped the loop, the pipe
	// stops draining and the test must fail on the missing reply, not hang.
	go func() {
		for _, line := range runnableMalformedFrames() {
			if _, err := io.WriteString(f.in, line+"\n"); err != nil {
				return
			}
		}
		_, _ = io.WriteString(f.in, `{"v":1,"type":"unserve"}`+"\n")
	}()
	// The loop is still alive and answers the next well-formed command.
	waitFrame(t, f.out, "unserved")
	if got := errorCodes(t, f.out); len(got) != 0 {
		t.Errorf("malformed frames must be dropped silently, got errors %v", got)
	}
	if len(f.out.ofType(t, "serving")) != 0 {
		t.Error("a malformed frame must never start serving")
	}
	f.send(t, `{"v":1,"type":"shutdown"}`)
	if code := f.exitCode(t); code != exitOK {
		t.Fatalf("exit %d, want 0", code)
	}
}

package main

import (
	"context"
	"errors"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strconv"
	"strings"
	"sync/atomic"
	"testing"
	"time"
)

func TestParentGoneRules(t *testing.T) {
	tests := []struct {
		name     string
		goos     string
		baseline int
		current  int
		want     bool
	}{
		{"macOS orphan under launchd", "darwin", 4242, 1, true},
		{"macOS started already orphaned", "darwin", 1, 1, true},
		{"macOS parent unchanged", "darwin", 4242, 4242, false},
		{"macOS debugger attached", "darwin", 4242, 9999, false},
		{"Linux reparented to init", "linux", 4242, 1, true},
		{"Linux reparented to a subreaper", "linux", 4242, 9999, true},
		{"Linux parent unchanged", "linux", 4242, 4242, false},
		{"Linux desktop is pid 1 in a namespace", "linux", 1, 1, false},
		{"Windows never reports", "windows", 4242, 1, false},
	}
	for _, tc := range tests {
		if got := parentGone(tc.goos, tc.baseline, tc.current); got != tc.want {
			t.Errorf("%s: parentGone(%q, %d, %d) = %v, want %v", tc.name, tc.goos, tc.baseline, tc.current, got, tc.want)
		}
	}
}

// newParentCheck must read the parent pid through getppid when it is called
// and later on every check, and apply the rule of the platform it runs on. Not
// parallel: getppid is shared.
func TestNewParentCheckAppliesThePlatformRule(t *testing.T) {
	if runtime.GOOS == "windows" {
		if newParentCheck() != nil {
			t.Fatal("Windows has no parent check")
		}
		return
	}
	fromOS := getppid
	t.Cleanup(func() { getppid = fromOS })
	darwin := runtime.GOOS == "darwin"
	tests := []struct {
		name     string
		baseline int
		current  int
		want     bool
	}{
		{"orphan under pid 1", 4242, 1, true},
		{"started already orphaned", 1, 1, darwin},
		{"parent replaced by another process", 4242, 9999, !darwin},
		{"parent unchanged", 4242, 4242, false},
	}
	for _, tc := range tests {
		pid := tc.baseline
		getppid = func() int { return pid }
		gone := newParentCheck()
		pid = tc.current
		if got := gone(); got != tc.want {
			t.Errorf("%s: parent %d then %d on %s: gone = %v, want %v", tc.name, tc.baseline, tc.current, runtime.GOOS, got, tc.want)
		}
	}
}

func TestWatchParent(t *testing.T) {
	fail := func() { t.Error("cancelled") }
	// Nothing to check: returns at once.
	watchParent(context.Background(), nil, time.Millisecond, fail)
	// A live parent: ends with its context and never cancels.
	ctx, stop := context.WithCancel(context.Background())
	done := make(chan struct{})
	go func() {
		watchParent(ctx, func() bool { return false }, time.Millisecond, fail)
		close(done)
	}()
	time.Sleep(20 * time.Millisecond)
	stop()
	select {
	case <-done:
	case <-time.After(5 * time.Second):
		t.Fatal("watchParent outlived its context")
	}
	// A gone parent cancels at once, without waiting for the first tick.
	cancelled := make(chan struct{})
	go watchParent(context.Background(), func() bool { return true }, time.Hour, func() { close(cancelled) })
	select {
	case <-cancelled:
	case <-time.After(5 * time.Second):
		t.Fatal("a parent that is already gone was not noticed at once")
	}
}

// A cancel before init (a signal, or a parent already gone) must not carry on
// into starting the node with an empty config.
func TestRunCancelledBeforeInitNeverStartsTheNode(t *testing.T) {
	reader, writer := io.Pipe()
	defer writer.Close()
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	code := run(ctx, runConfig{in: reader, out: &syncBuffer{}, diag: discard, initTimeout: 5 * time.Second, closeGrace: 200 * time.Millisecond,
		newTailnet: func(dir, hostname string) (tailnet, error) {
			t.Errorf("the node was created for %q %q", dir, hostname)
			return nil, errors.New("must not start")
		}})
	if code != exitOK {
		t.Fatalf("exit %d, want 0", code)
	}
}

// stdin stays open in both tests: only the parent check can end the run.
func TestRunStopsWhenTheParentIsGoneBeforeInit(t *testing.T) {
	var gone atomic.Bool
	f := startRun(t, func(cfg *runConfig, _ *fakeTailnet) {
		cfg.parentGone = gone.Load
		cfg.parentPoll = 5 * time.Millisecond
	})
	time.Sleep(50 * time.Millisecond)
	select {
	case code := <-f.result:
		t.Fatalf("run ended with %d while the parent was alive", code)
	default:
	}
	gone.Store(true)
	if code := f.exitCode(t); code != exitOK {
		t.Fatalf("exit %d, want 0", code)
	}
	if got := errorCodes(t, f.out); len(got) != 0 {
		t.Fatalf("error codes %v", got)
	}
	if f.host != "" {
		t.Fatal("the node was created")
	}
}

func TestRunStopsWhenTheParentIsGoneAfterInit(t *testing.T) {
	var gone atomic.Bool
	f := startRun(t, func(cfg *runConfig, _ *fakeTailnet) {
		cfg.parentGone = gone.Load
		cfg.parentPoll = 5 * time.Millisecond
	})
	f.init(t)
	waitFrame(t, f.out, "state")
	select {
	case code := <-f.result:
		t.Fatalf("run ended with %d while the parent was alive", code)
	default:
	}
	gone.Store(true)
	if code := f.exitCode(t); code != exitOK {
		t.Fatalf("exit %d, want 0", code)
	}
	select {
	case <-f.ts.closed:
	default:
		t.Fatal("the node was not closed")
	}
}

// Real getppid: a helper process (this test binary again) started in the
// background of a shell that then exits sees its parent go.
func TestOrphanedProcessSeesItsParentGo(t *testing.T) {
	dir := os.Getenv("YZC_ORPHAN_DIR")
	if os.Getenv("YZC_ORPHAN_HELPER") == "1" {
		gone := newParentCheck()
		touch(t, filepath.Join(dir, "ready"))
		ctx, cancel := context.WithCancel(context.Background())
		defer cancel()
		go watchParent(ctx, gone, 20*time.Millisecond, cancel)
		select {
		case <-ctx.Done():
			touch(t, filepath.Join(dir, "gone"))
		case <-time.After(10 * time.Second):
		}
		return
	}
	if runtime.GOOS == "windows" {
		t.Skip("Windows has no parent check, and no sh")
	}
	dir = t.TempDir()
	exists := func(name string) func() bool {
		return func() bool { _, err := os.Stat(filepath.Join(dir, name)); return err == nil }
	}
	// The loop ends at the gate, or by itself after 20 s for a test that dies
	// without running its cleanup (a go test timeout).
	shell := exec.Command("sh", "-c", `"$YZC_BIN" -test.run='^TestOrphanedProcessSeesItsParentGo$' >/dev/null 2>&1 & echo $! >"$YZC_PID"; i=0; while [ ! -e "$YZC_GATE" ] && [ "$i" -lt 400 ]; do i=$((i+1)); sleep 0.05; done`)
	shell.Env = append(os.Environ(), "YZC_ORPHAN_HELPER=1", "YZC_ORPHAN_DIR="+dir, "YZC_BIN="+os.Args[0], "YZC_GATE="+filepath.Join(dir, "gate"), "YZC_PID="+filepath.Join(dir, "pid"))
	if err := shell.Start(); err != nil {
		t.Fatal(err)
	}
	// A failure before the gate is touched would leave the shell looping for
	// good, and its helper with it. Once the shell is dead the pid file is
	// final. A passed test has nothing left to kill.
	t.Cleanup(func() {
		_ = shell.Process.Kill()
		_ = shell.Wait()
		if !t.Failed() {
			return
		}
		raw, err := os.ReadFile(filepath.Join(dir, "pid"))
		if err != nil {
			return
		}
		if pid, err := strconv.Atoi(strings.TrimSpace(string(raw))); err == nil {
			if helper, err := os.FindProcess(pid); err == nil {
				_ = helper.Kill()
			}
		}
	})
	waitFor(t, "the helper to record its parent", exists("ready"))
	time.Sleep(100 * time.Millisecond)
	if exists("gone")() {
		t.Fatal("the helper saw its parent go while the shell was alive")
	}
	touch(t, filepath.Join(dir, "gate"))
	if err := shell.Wait(); err != nil {
		t.Fatal(err)
	}
	waitFor(t, "the orphan to see its parent go", exists("gone"))
}

// The real main() as an orphan, with nothing but the parent check to stop it:
// its stdin stays open (the test keeps the write end, like a leaked copy of
// the desktop's pipe) and no init frame ever comes. Only the parent check can
// end it before the init timeout, and it ends it without the error frame the
// timeout would write. The test is not the helper's parent, so it sees the
// exit as EOF on a pipe only the helper holds.
func TestMainStopsAnOrphanWhoseStdinIsStillOpen(t *testing.T) {
	dir := os.Getenv("YZC_MAIN_DIR")
	if os.Getenv("YZC_MAIN_HELPER") == "1" {
		touch(t, filepath.Join(dir, "ready"))
		if info, err := os.Stdin.Stat(); err != nil || info.Mode()&os.ModeNamedPipe == 0 {
			t.Fatalf("stdin is not the pipe the test holds open (stat error: %v)", err)
		}
		main()
		return
	}
	if runtime.GOOS == "windows" {
		t.Skip("Windows has no parent check, and no sh")
	}
	dir = t.TempDir()
	exists := func(name string) func() bool {
		return func() bool { _, err := os.Stat(filepath.Join(dir, name)); return err == nil }
	}
	stdinRead, stdinWrite, err := os.Pipe()
	if err != nil {
		t.Fatal(err)
	}
	outRead, outWrite, err := os.Pipe()
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		for _, f := range []*os.File{stdinRead, stdinWrite, outRead, outWrite} {
			_ = f.Close()
		}
	})
	// A background command of a non-interactive sh has /dev/null for stdin
	// before its own redirections apply, so the stdin pipe comes in as fd 3 and
	// is moved onto fd 0 explicitly. Only the test holds its write end
	// (os.Pipe ends are close-on-exec), so EOF never arrives. stdout and stderr
	// of the helper are the output pipe, passed as fd 4; the shell closes its
	// own fd 3 and 4 right after the launch, so the helper is the only holder
	// of the write end and EOF on the pipe is the helper's exit.
	// The loop ends at the gate, or by itself after 20 s for a test that dies
	// without running its cleanup (a go test timeout).
	shell := exec.Command("sh", "-c", `"$YZC_BIN" -test.run='^TestMainStopsAnOrphanWhoseStdinIsStillOpen$' <&3 >&4 2>&4 3<&- 4<&- & echo $! >"$YZC_PID"; exec 3<&- 4<&-; i=0; while [ ! -e "$YZC_GATE" ] && [ "$i" -lt 400 ]; do i=$((i+1)); sleep 0.05; done`)
	shell.Env = append(os.Environ(), "YZC_MAIN_HELPER=1", "YZC_MAIN_DIR="+dir, "YZC_BIN="+os.Args[0], "YZC_GATE="+filepath.Join(dir, "gate"), "YZC_PID="+filepath.Join(dir, "pid"))
	shell.ExtraFiles = []*os.File{stdinRead, outWrite}
	if err := shell.Start(); err != nil {
		t.Fatal(err)
	}
	// The test's own copies would keep the pipes alive after the helper is gone.
	_ = stdinRead.Close()
	_ = outWrite.Close()
	// A failure before the gate is touched would leave the shell looping for
	// good, and its helper with it. Once the shell is dead the pid file is
	// final. A passed test has nothing left to kill.
	t.Cleanup(func() {
		_ = shell.Process.Kill()
		_ = shell.Wait()
		if !t.Failed() {
			return
		}
		raw, err := os.ReadFile(filepath.Join(dir, "pid"))
		if err != nil {
			return
		}
		if pid, err := strconv.Atoi(strings.TrimSpace(string(raw))); err == nil {
			if helper, err := os.FindProcess(pid); err == nil {
				_ = helper.Kill()
			}
		}
	})
	exited := make(chan []byte, 1)
	go func() {
		out, _ := io.ReadAll(outRead)
		exited <- out
	}()
	waitFor(t, "the helper to reach main", exists("ready"))
	// main's first statement records the parent pid a few instructions after
	// the signal; the settle and the shell's own poll of the gate leave that
	// ample time before the shell can exit.
	time.Sleep(100 * time.Millisecond)
	select {
	case out := <-exited:
		t.Fatalf("the helper ended while its parent was alive, output %q", out)
	default:
	}
	touch(t, filepath.Join(dir, "gate"))
	// The shell is gone within 50 ms of the gate and the parent check sees it
	// within parentPoll (250 ms). Without the check the helper waits out
	// initTimeout (5 s, counted from main's start, some 100 ms before the gate)
	// and then writes an error frame. 2 s is far from both.
	deadline := time.After(2 * time.Second)
	if err := shell.Wait(); err != nil {
		t.Fatal(err)
	}
	select {
	case out := <-exited:
		if len(out) != 0 {
			t.Fatalf("the helper wrote %q: it ended through the init timeout or stdin, not the parent check", out)
		}
	case <-deadline:
		t.Fatal("the orphan was still running 2 s after its parent went")
	}
}

func touch(t *testing.T, path string) {
	t.Helper()
	if err := os.WriteFile(path, nil, 0o600); err != nil {
		t.Fatal(err)
	}
}

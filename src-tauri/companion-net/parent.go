package main

import (
	"context"
	"os"
	"runtime"
	"time"
)

// parentGone says whether the desktop that started this process is gone, from
// the parent pid seen at start and the one seen now. goos is a parameter so a
// test can check every platform's rule on any host.
//
// macOS hands every orphan to launchd (pid 1) and has no subreapers, and a
// debugger that attaches also becomes the parent, which must not stop the
// process. Elsewhere an orphan goes to init or the nearest subreaper, so any
// change from the pid seen at start counts, but a start under pid 1 can be a
// live desktop in a pid namespace. Windows keeps the parent id it was started
// with forever, so it never reports anything.
func parentGone(goos string, baseline, current int) bool {
	switch goos {
	case "darwin":
		return current == 1
	case "windows":
		return false
	default:
		return current != baseline
	}
}

// getppid is a variable so a test can feed newParentCheck any parent pid.
var getppid = os.Getppid

// newParentCheck records the parent pid now, so call it first thing in main:
// the init frame may arrive five seconds later. It returns nil where there is
// nothing to check.
func newParentCheck() func() bool {
	if runtime.GOOS == "windows" {
		return nil
	}
	baseline := getppid()
	return func() bool { return parentGone(runtime.GOOS, baseline, getppid()) }
}

// watchParent calls cancel as soon as gone reports true, and stops when ctx is
// done. A nil gone watches nothing.
func watchParent(ctx context.Context, gone func() bool, every time.Duration, cancel context.CancelFunc) {
	if gone == nil {
		return
	}
	tick := time.NewTicker(every)
	defer tick.Stop()
	for {
		if gone() {
			cancel()
			return
		}
		select {
		case <-ctx.Done():
			return
		case <-tick.C:
		}
	}
}

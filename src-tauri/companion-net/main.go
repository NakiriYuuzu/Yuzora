// yuzora-companion-net is the Companion's embedded Tailscale node. The desktop
// app starts it and talks NDJSON over stdin/stdout (see CONTRACT in the plan);
// it terminates TLS on the node's ts.net name, admits only the owner's
// devices and proxies them to the loopback gateway.
package main

import (
	"cmp"
	"context"
	"errors"
	"io"
	"log"
	"os"
	"os/signal"
	"syscall"
	"time"

	"tailscale.com/envknob"
)

// version is the desktop version this binary was built for (-ldflags -X).
var version = "dev"

const (
	exitOK       = 0
	exitFailure  = 1
	exitProtocol = 2

	initTimeout = 5 * time.Second
	// parentPoll is faster than the gateway's 1 s check, so the node is unserved
	// before the gateway (which then waits 500 ms) lets go of its loopback port.
	parentPoll = 250 * time.Millisecond
	// closeGrace keeps shutdown under the 2 s the desktop allows.
	closeGrace = 1500 * time.Millisecond
)

type runConfig struct {
	in          io.Reader
	out         io.Writer
	diag        *log.Logger
	newTailnet  func(dir, hostname string) (tailnet, error)
	initTimeout time.Duration
	closeGrace  time.Duration
	// parentGone reports that the desktop is gone though stdin is still open
	// (nil: not checked), looked at every parentPoll.
	parentGone func() bool
	parentPoll time.Duration
}

// setupProcess runs before tsnet or any goroutine exists (newParentCheck only
// records the parent pid).
func setupProcess() {
	// tsnet uploads the node's logs (peer names, the tailnet name, the login
	// URL) to log.tailscale.com unless this is set. Nothing may leave the machine.
	envknob.SetNoLogsNoSupport()
	// Dependencies print through the std logger: tshttpproxy prints the proxy
	// URL, userinfo included. It defaults to stderr, which the desktop logs, so
	// it is discarded until newTSNetNode points it at <stateDir>/tsnet.log.
	log.SetOutput(io.Discard)
}

func main() {
	parentGone := newParentCheck()
	setupProcess()
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	code := run(ctx, runConfig{
		in:          os.Stdin,
		out:         os.Stdout,
		diag:        log.New(os.Stderr, "yuzora-companion-net: ", log.LstdFlags),
		newTailnet:  newTSNetNode,
		initTimeout: initTimeout,
		closeGrace:  closeGrace,
		parentGone:  parentGone,
		parentPoll:  parentPoll,
	})
	stop()
	os.Exit(code)
}

// inbound is one thing read from stdin: a parsed command, a frame that did
// not parse (the stream is still usable) or a read error (it is not).
type inbound struct {
	cmd      command
	parseErr error
	readErr  error
}

// run is the whole process: init handshake, node start, command loop, shutdown.
func run(ctx context.Context, cfg runConfig) int {
	// A desktop that died without closing stdin (a leaked copy of the pipe keeps
	// it open) ends the run like EOF does, through ctx.
	ctx, parentCancel := context.WithCancel(ctx)
	defer parentCancel()
	go watchParent(ctx, cfg.parentGone, cfg.parentPoll, parentCancel)

	emit := &emitter{w: cfg.out, diag: cfg.diag}
	frames := make(chan inbound)
	stopReading := make(chan struct{})
	defer close(stopReading)
	go func() {
		reader := newFrameReader(cfg.in)
		for {
			var msg inbound
			line, err := reader.next()
			if err != nil {
				msg.readErr = err
			} else {
				msg.cmd, msg.parseErr = parseCommand(line)
			}
			select {
			case frames <- msg:
			case <-stopReading:
				return
			}
			if msg.readErr != nil {
				return
			}
		}
	}()

	initCfg, code := handshake(ctx, cfg, emit, frames)
	if code != exitOK {
		return code
	}
	// A cancel during the handshake returns exitOK without an init config.
	if ctx.Err() != nil {
		return exitOK
	}
	ts, err := cfg.newTailnet(initCfg.stateDir, initCfg.hostname)
	if err != nil {
		emit.fail("net-start-failed", err)
		return exitFailure
	}
	emit.send("started", map[string]any{"version": version})
	if err := ts.Start(); err != nil {
		emit.fail("net-start-failed", err)
		closeWithin(ts, cfg.closeGrace)
		return exitFailure
	}
	lc, err := ts.LocalClient()
	if err != nil {
		emit.fail("net-start-failed", err)
		closeWithin(ts, cfg.closeGrace)
		return exitFailure
	}

	nodeCtx, cancel := context.WithCancel(ctx)
	n := newNode(emit, cfg.diag, ts, lc)
	go n.pollLoop(nodeCtx)

	code = commandLoop(nodeCtx, cfg, n, frames)
	cancel()
	n.stopServing()
	closeWithin(ts, cfg.closeGrace)
	return code
}

// handshake waits for the init frame and validates it.
func handshake(ctx context.Context, cfg runConfig, emit *emitter, frames <-chan inbound) (initConfig, int) {
	timer := time.NewTimer(cfg.initTimeout)
	defer timer.Stop()
	var msg inbound
	select {
	case msg = <-frames:
	case <-timer.C:
		emit.fail("net-init-invalid", errors.New("no init frame within 5 s"))
		return initConfig{}, exitProtocol
	case <-ctx.Done():
		return initConfig{}, exitOK
	}
	err := cmp.Or(msg.readErr, msg.parseErr)
	if err == nil && msg.cmd.Type != "init" {
		err = errors.New("the first frame must be init")
	}
	if err != nil {
		emit.fail("net-init-invalid", err)
		return initConfig{}, exitProtocol
	}
	initCfg, code, err := validateInit(msg.cmd, version)
	if err != nil {
		emit.fail(code, err)
		return initConfig{}, exitProtocol
	}
	return initCfg, exitOK
}

// commandLoop runs desktop commands until shutdown, stdin EOF, a signal or a
// logout, and returns the exit code.
func commandLoop(ctx context.Context, cfg runConfig, n *node, frames <-chan inbound) int {
	for {
		select {
		case <-ctx.Done():
			return exitOK
		case msg := <-frames:
			switch {
			case errors.Is(msg.readErr, io.EOF):
				return exitOK
			case msg.readErr != nil:
				cfg.diag.Printf("stopping: %v", msg.readErr)
				return exitProtocol
			case msg.parseErr != nil:
				cfg.diag.Printf("ignoring frame: %v", msg.parseErr)
				continue
			}
			switch msg.cmd.Type {
			case "serve":
				n.serve(msg.cmd)
			case "unserve":
				n.unserve()
			case "logout":
				if n.logout(ctx) {
					return exitOK
				}
			case "shutdown":
				return exitOK
			default:
				cfg.diag.Printf("ignoring unexpected %q frame", msg.cmd.Type)
			}
		}
	}
}

// closeWithin closes the node but gives up after the grace period: tsnet's own
// shutdown can take seconds when the network is gone, and the process exit
// frees whatever is left.
func closeWithin(ts tailnet, grace time.Duration) {
	done := make(chan struct{})
	go func() {
		_ = ts.Close()
		close(done)
	}()
	select {
	case <-done:
	case <-time.After(grace):
	}
}

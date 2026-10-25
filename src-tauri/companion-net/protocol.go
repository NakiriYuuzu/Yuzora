package main

import (
	"bufio"
	"bytes"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"log"
	"path/filepath"
	"regexp"
	"strconv"
	"sync"
	"unicode"
)

const (
	protocolVersion = 1
	// maxFrameBytes bounds one NDJSON line in either direction, newline excluded.
	maxFrameBytes = 64 << 10
)

var errFrameTooLong = errors.New("frame exceeds 64 KiB")

// frameReader reads NDJSON lines and rejects any longer than maxFrameBytes.
type frameReader struct{ r *bufio.Reader }

func newFrameReader(r io.Reader) *frameReader {
	// One byte more than a full frame so the newline fits in the buffer.
	return &frameReader{bufio.NewReaderSize(r, maxFrameBytes+1)}
}

// next returns the next non-empty line; a final line without a newline counts.
func (f *frameReader) next() ([]byte, error) {
	for {
		line, err := f.r.ReadSlice('\n')
		if errors.Is(err, bufio.ErrBufferFull) {
			return nil, errFrameTooLong
		}
		if err != nil && (!errors.Is(err, io.EOF) || len(line) == 0) {
			return nil, err
		}
		if line = bytes.TrimSpace(line); len(line) > 0 {
			return bytes.Clone(line), nil
		}
		if err != nil {
			return nil, err
		}
	}
}

// command is a desktop-to-net frame; which fields apply depends on Type.
type command struct {
	V             int    `json:"v"`
	Type          string `json:"type"`
	StateDir      string `json:"stateDir"`
	Hostname      string `json:"hostname"`
	Version       string `json:"version"`
	Target        string `json:"target"`
	ProxySecret   string `json:"proxySecret"`
	RequiredLogin string `json:"requiredLogin"`
}

func parseCommand(line []byte) (command, error) {
	var cmd command
	if err := json.Unmarshal(line, &cmd); err != nil {
		return command{}, fmt.Errorf("malformed frame: %w", err)
	}
	if cmd.V != protocolVersion {
		return command{}, fmt.Errorf("unsupported protocol version %d", cmd.V)
	}
	if cmd.Type == "" {
		return command{}, errors.New("frame has no type")
	}
	return cmd, nil
}

var (
	hostnamePattern    = regexp.MustCompile(`^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$`)
	serveTargetPattern = regexp.MustCompile(`^http://127\.0\.0\.1:([1-9][0-9]{0,4})$`)
)

// initConfig is a validated init frame.
type initConfig struct {
	stateDir string
	hostname string
}

// validateInit checks an init frame against this binary's version. The version
// is compared first so a desktop speaking another revision gets the mismatch
// code instead of an invalid-field one.
func validateInit(cmd command, binaryVersion string) (initConfig, string, error) {
	if cmd.Version == "" {
		return initConfig{}, "net-init-invalid", errors.New("init has no version")
	}
	if cmd.Version != binaryVersion {
		return initConfig{}, "net-version-mismatch", fmt.Errorf("desktop is %s but yuzora-companion-net is %s", cmd.Version, binaryVersion)
	}
	if cmd.StateDir == "" || !filepath.IsAbs(cmd.StateDir) {
		return initConfig{}, "net-init-invalid", errors.New("stateDir must be an absolute path")
	}
	if !hostnamePattern.MatchString(cmd.Hostname) {
		return initConfig{}, "net-init-invalid", errors.New("hostname must be a lowercase DNS label")
	}
	return initConfig{stateDir: cmd.StateDir, hostname: cmd.Hostname}, "", nil
}

// serveConfig is a validated serve frame.
type serveConfig struct {
	target        string
	proxySecret   string
	requiredLogin string
}

func validateServe(cmd command) (serveConfig, error) {
	match := serveTargetPattern.FindStringSubmatch(cmd.Target)
	if match == nil {
		return serveConfig{}, errors.New("target must be http://127.0.0.1:<port>")
	}
	if port, _ := strconv.Atoi(match[1]); port > 65535 {
		return serveConfig{}, errors.New("target port is out of range")
	}
	if len(cmd.ProxySecret) != 43 {
		return serveConfig{}, errors.New("proxySecret must be 43 characters")
	}
	secret, err := base64.RawURLEncoding.Strict().DecodeString(cmd.ProxySecret)
	if err != nil || len(secret) != 32 {
		return serveConfig{}, errors.New("proxySecret must be 32 bytes of unpadded base64url")
	}
	if cmd.RequiredLogin == "" {
		return serveConfig{}, errors.New("requiredLogin is empty")
	}
	for _, r := range cmd.RequiredLogin {
		if unicode.IsControl(r) {
			return serveConfig{}, errors.New("requiredLogin has control characters")
		}
	}
	return serveConfig{target: cmd.Target, proxySecret: cmd.ProxySecret, requiredLogin: cmd.RequiredLogin}, nil
}

// emitter writes net-to-desktop frames; one line per Write, serialised.
type emitter struct {
	mu   sync.Mutex
	w    io.Writer
	diag *log.Logger
}

func (e *emitter) send(typ string, fields map[string]any) {
	frame := map[string]any{}
	for key, value := range fields {
		frame[key] = value
	}
	frame["v"] = protocolVersion
	frame["type"] = typ
	var line bytes.Buffer
	encoder := json.NewEncoder(&line)
	encoder.SetEscapeHTML(false)
	if err := encoder.Encode(frame); err != nil {
		e.diag.Printf("dropping %s frame: %v", typ, err)
		return
	}
	if line.Len() > maxFrameBytes+1 {
		e.diag.Printf("dropping %s frame: %d bytes", typ, line.Len())
		return
	}
	e.mu.Lock()
	defer e.mu.Unlock()
	if _, err := e.w.Write(line.Bytes()); err != nil {
		e.diag.Printf("writing %s frame: %v", typ, err)
	}
}

func (e *emitter) fail(code string, err error) {
	e.send("error", map[string]any{"code": code, "message": err.Error()})
}

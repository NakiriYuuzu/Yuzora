package main

import (
	"errors"
	"io"
	"path/filepath"
	"strings"
	"testing"
)

func TestFrameReader(t *testing.T) {
	reader := newFrameReader(strings.NewReader("{\"a\":1}\r\n\n  \n{\"b\":2}\n{\"c\":3}"))
	for _, want := range []string{`{"a":1}`, `{"b":2}`, `{"c":3}`} {
		line, err := reader.next()
		if err != nil || string(line) != want {
			t.Fatalf("next() = %q, %v; want %q", line, err, want)
		}
	}
	if _, err := reader.next(); !errors.Is(err, io.EOF) {
		t.Fatalf("after the last frame: %v, want EOF", err)
	}
}

func TestFrameReaderLimit(t *testing.T) {
	exact := strings.Repeat("x", maxFrameBytes)
	reader := newFrameReader(strings.NewReader(exact + "\nnext\n"))
	if line, err := reader.next(); err != nil || len(line) != maxFrameBytes {
		t.Fatalf("a 64 KiB frame must pass: len=%d err=%v", len(line), err)
	}
	if line, err := reader.next(); err != nil || string(line) != "next" {
		t.Fatalf("frame after the limit: %q, %v", line, err)
	}

	over := newFrameReader(strings.NewReader(exact + "x\n"))
	if _, err := over.next(); !errors.Is(err, errFrameTooLong) {
		t.Fatalf("a frame over 64 KiB: %v, want errFrameTooLong", err)
	}
	unterminated := newFrameReader(strings.NewReader(exact + "xx"))
	if _, err := unterminated.next(); !errors.Is(err, errFrameTooLong) {
		t.Fatalf("an unterminated frame over 64 KiB: %v, want errFrameTooLong", err)
	}
}

func TestParseCommand(t *testing.T) {
	cmd, err := parseCommand([]byte(`{"v":1,"type":"serve","target":"http://127.0.0.1:1","extra":true}`))
	if err != nil || cmd.Type != "serve" || cmd.Target != "http://127.0.0.1:1" {
		t.Fatalf("valid frame: %+v, %v", cmd, err)
	}
	for name, line := range map[string]string{
		"not json":         `init`,
		"no version":       `{"type":"init"}`,
		"other version":    `{"v":2,"type":"init"}`,
		"version string":   `{"v":"1","type":"init"}`,
		"no type":          `{"v":1}`,
		"not an object":    `[1]`,
		"field wrong type": `{"v":1,"type":"init","stateDir":5}`,
	} {
		if _, err := parseCommand([]byte(line)); err == nil {
			t.Errorf("%s: accepted %s", name, line)
		}
	}
}

func TestValidateInit(t *testing.T) {
	// Absolute on every OS: a literal "/tmp/..." has no drive letter on Windows.
	stateDir := filepath.Join(t.TempDir(), "net")
	good := command{Type: "init", StateDir: stateDir, Hostname: "yuzora", Version: "0.0.19"}
	cfg, code, err := validateInit(good, "0.0.19")
	if err != nil || code != "" || cfg.stateDir != stateDir || cfg.hostname != "yuzora" {
		t.Fatalf("valid init: %+v %q %v", cfg, code, err)
	}

	tests := []struct {
		name   string
		mutate func(*command)
		binary string
		code   string
	}{
		{"missing stateDir", func(c *command) { c.StateDir = "" }, "0.0.19", "net-init-invalid"},
		{"relative stateDir", func(c *command) { c.StateDir = "state/net" }, "0.0.19", "net-init-invalid"},
		{"dot stateDir", func(c *command) { c.StateDir = "." }, "0.0.19", "net-init-invalid"},
		{"missing hostname", func(c *command) { c.Hostname = "" }, "0.0.19", "net-init-invalid"},
		{"uppercase hostname", func(c *command) { c.Hostname = "Yuzora" }, "0.0.19", "net-init-invalid"},
		{"dotted hostname", func(c *command) { c.Hostname = "a.b" }, "0.0.19", "net-init-invalid"},
		{"missing version", func(c *command) { c.Version = "" }, "0.0.19", "net-init-invalid"},
		{"version mismatch", func(c *command) { c.Version = "0.0.18" }, "0.0.19", "net-version-mismatch"},
		{"version mismatch wins over a bad field", func(c *command) { c.Version = "0.0.18"; c.StateDir = "" }, "0.0.19", "net-version-mismatch"},
		{"dev binary vs release desktop", func(c *command) {}, "dev", "net-version-mismatch"},
	}
	for _, tc := range tests {
		cmd := good
		tc.mutate(&cmd)
		if _, code, err := validateInit(cmd, tc.binary); err == nil || code != tc.code {
			t.Errorf("%s: code=%q err=%v, want %q", tc.name, code, err, tc.code)
		}
	}
}

func TestValidateServe(t *testing.T) {
	good := command{Type: "serve", Target: "http://127.0.0.1:47110", ProxySecret: testSecret, RequiredLogin: testLogin}
	cfg, err := validateServe(good)
	if err != nil || cfg.target != good.Target || cfg.proxySecret != testSecret || cfg.requiredLogin != testLogin {
		t.Fatalf("valid serve: %+v %v", cfg, err)
	}
	for _, target := range []string{"http://127.0.0.1:1", "http://127.0.0.1:65535"} {
		cmd := good
		cmd.Target = target
		if _, err := validateServe(cmd); err != nil {
			t.Errorf("target %s rejected: %v", target, err)
		}
	}

	badTargets := []string{
		"", "http://localhost:47110", "http://[::1]:47110", "http://127.0.0.2:47110", "http://0.0.0.0:47110",
		"https://127.0.0.1:47110", "http://127.0.0.1", "http://127.0.0.1:", "http://127.0.0.1:47110/",
		"http://127.0.0.1:47110/path", "http://127.0.0.1:47110?x=1", "http://127.0.0.1:0", "http://127.0.0.1:65536",
		"http://127.0.0.1:99999", "http://127.0.0.1:100000", "http://127.0.0.1:047110", "http://127.0.0.1:-1",
		"http://user@127.0.0.1:47110", "http://127.0.0.1:47110\n", " http://127.0.0.1:47110", "HTTP://127.0.0.1:47110",
	}
	for _, target := range badTargets {
		cmd := good
		cmd.Target = target
		if _, err := validateServe(cmd); err == nil {
			t.Errorf("target %q accepted", target)
		}
	}

	badSecrets := map[string]string{
		"empty":           "",
		"short":           testSecret[:42],
		"long":            testSecret + "A",
		"padded":          testSecret[:42] + "=",
		"standard base64": strings.Repeat("+", 43),
		"slash":           strings.Repeat("/", 43),
		"space":           strings.Repeat(" ", 43),
		"non-canonical":   strings.Repeat("B", 43),
		"non-ascii":       strings.Repeat("é", 22)[:43],
		"newline":         testSecret[:42] + "\n",
	}
	for name, secret := range badSecrets {
		cmd := good
		cmd.ProxySecret = secret
		if _, err := validateServe(cmd); err == nil {
			t.Errorf("secret %s accepted", name)
		}
	}

	for _, login := range []string{"", "a\tb", "a\nb", "a\rb", "a\x00b", "a\x7fb", "a\u0085b"} {
		cmd := good
		cmd.RequiredLogin = login
		if _, err := validateServe(cmd); err == nil {
			t.Errorf("login %q accepted", login)
		}
	}
	for _, login := range []string{"owner@example.com", "ünï@example.com", "山田@example.jp"} {
		cmd := good
		cmd.RequiredLogin = login
		if _, err := validateServe(cmd); err != nil {
			t.Errorf("login %q rejected: %v", login, err)
		}
	}
}

func TestEmitterFrames(t *testing.T) {
	emit, out := newTestEmitter()
	emit.send("running", map[string]any{"dnsName": "a.ts.net", "login": "a<b>&c@x\nz", "type": "spoof", "v": 9})
	emit.send("unserved", nil)
	emit.fail("net-not-ready", errors.New("not ready"))

	frames := out.frames(t)
	if len(frames) != 3 {
		t.Fatalf("got %d frames", len(frames))
	}
	if frames[0]["type"] != "running" || frames[0]["v"] != float64(1) || frames[0]["login"] != "a<b>&c@x\nz" {
		t.Errorf("fields must not override v/type and must round-trip: %v", frames[0])
	}
	if raw := out.buf.String(); strings.Contains(raw, `\u003c`) || strings.Count(raw, "\n") != 3 {
		t.Errorf("one line per frame, no HTML escaping: %q", raw)
	}
	if frames[2]["code"] != "net-not-ready" || frames[2]["message"] != "not ready" {
		t.Errorf("error frame: %v", frames[2])
	}
}

func TestEmitterDropsOversizedFrames(t *testing.T) {
	emit, out := newTestEmitter()
	emit.send("login-required", map[string]any{"authUrl": strings.Repeat("a", maxFrameBytes)})
	emit.send("started", map[string]any{"version": "x"})
	if got := out.types(t); len(got) != 1 || got[0] != "started" {
		t.Fatalf("frames after an oversized one: %v", got)
	}
}

// malformedFrames are lines the desktop channel must never turn into a command.
func malformedFrames() map[string]string {
	deepArray := strings.Repeat("[", 100000) + strings.Repeat("]", 100000)
	deepObject := strings.Repeat(`{"a":`, 100000) + "0" + strings.Repeat("}", 100000)
	return map[string]string{
		"empty":                   ``,
		"truncated":               `{"v":1,"type":"serve"`,
		"trailing garbage":        `{"v":1,"type":"shutdown"} x`,
		"two objects":             `{"v":1,"type":"shutdown"}{"v":1,"type":"shutdown"}`,
		"null":                    `null`,
		"number":                  `1`,
		"string":                  `"shutdown"`,
		"true":                    `true`,
		"empty object":            `{}`,
		"positional array":        `[1,"shutdown"]`,
		"version float":           `{"v":1.5,"type":"shutdown"}`,
		"version 1.0":             `{"v":1.0,"type":"shutdown"}`,
		"version exponent":        `{"v":1e0,"type":"shutdown"}`,
		"version bool":            `{"v":true,"type":"shutdown"}`,
		"version null":            `{"v":null,"type":"shutdown"}`,
		"version array":           `{"v":[1],"type":"shutdown"}`,
		"version object":          `{"v":{},"type":"shutdown"}`,
		"version negative":        `{"v":-1,"type":"shutdown"}`,
		"version overflow":        `{"v":9223372036854775808,"type":"shutdown"}`,
		"type number":             `{"v":1,"type":5}`,
		"type null":               `{"v":1,"type":null}`,
		"type bool":               `{"v":1,"type":true}`,
		"type array":              `{"v":1,"type":["shutdown"]}`,
		"type object":             `{"v":1,"type":{}}`,
		"type empty":              `{"v":1,"type":""}`,
		"field array":             `{"v":1,"type":"serve","target":["http://127.0.0.1:1"]}`,
		"field object":            `{"v":1,"type":"serve","proxySecret":{}}`,
		"field bool":              `{"v":1,"type":"init","hostname":false}`,
		"raw NUL":                 "\x00",
		"NUL after object":        "{\"v\":1,\"type\":\"shutdown\"}\x00",
		"NUL inside string":       "{\"v\":1,\"type\":\"shut\x00down\"}",
		"nesting in ignored key":  `{"v":1,"type":"shutdown","x":` + deepArray + `}`,
		"nesting in object":       `{"v":1,"type":"shutdown","x":` + deepObject + `}`,
		"nesting at top level":    deepArray,
		"nesting in known field":  `{"v":1,"type":"init","stateDir":` + deepArray + `}`,
		"unterminated deep":       strings.Repeat("[", 100000),
		"BOM":                     "\xef\xbb\xbf" + `{"v":1,"type":"shutdown"}`,
		"unquoted keys":           `{v:1,type:"shutdown"}`,
		"single quotes":           `{'v':1,'type':'shutdown'}`,
		"comment":                 `{"v":1,"type":"shutdown"}//x`,
		"NaN":                     `{"v":NaN,"type":"shutdown"}`,
		"control char in string":  "{\"v\":1,\"type\":\"a\tb\"}",
		"lone surrogate escape":   `{"v":1,"type":"\ud800"}`,
		"bad escape":              `{"v":1,"type":"\q"}`,
		"invalid UTF-8 in object": "{\"v\":1,\"type\":\"shutdown\"}\xff",
	}
}

func TestParseCommandMalformedTable(t *testing.T) {
	for name, line := range malformedFrames() {
		cmd, err := parseCommand([]byte(line))
		// A lone surrogate escape decodes to U+FFFD: a well-formed but unknown type.
		if name == "lone surrogate escape" {
			if err != nil || cmd.Type != "�" {
				t.Errorf("%s: %+v %v", name, cmd, err)
			}
			continue
		}
		if err == nil {
			t.Errorf("%s: accepted as %+v", name, cmd)
		}
	}
}

// encoding/json is lenient in three ways the table above does not cover. They are
// pinned because the channel is the private stdin pipe from the desktop (which
// builds every frame with serde and never repeats a key); the gateway, which
// faces the tailnet, rejects duplicates instead (companion tests/admin.rs).
func TestParseCommandPinnedLeniencyOfTheTrustedPipe(t *testing.T) {
	// Invalid UTF-8 inside a string becomes U+FFFD rather than an error.
	cmd, err := parseCommand([]byte("{\"v\":1,\"type\":\"x\xffy\"}"))
	if err != nil || cmd.Type != "x�y" {
		t.Errorf("invalid UTF-8: %+v %v", cmd, err)
	}
	// Duplicate keys: the last value wins, for the version and the type alike.
	cmd, err = parseCommand([]byte(`{"v":1,"type":"init","type":"serve"}`))
	if err != nil || cmd.Type != "serve" {
		t.Errorf("duplicate type: %+v %v", cmd, err)
	}
	if _, err = parseCommand([]byte(`{"v":1,"v":2,"type":"init"}`)); err == nil {
		t.Error("duplicate version: the last (unsupported) value must win")
	}
	// Keys match case-insensitively.
	cmd, err = parseCommand([]byte(`{"V":1,"TYPE":"shutdown"}`))
	if err != nil || cmd.Type != "shutdown" {
		t.Errorf("case-insensitive keys: %+v %v", cmd, err)
	}
	// Unknown fields are ignored (forward compatible), nested ones included.
	cmd, err = parseCommand([]byte(`{"v":1,"type":"shutdown","future":{"a":[1,{"b":null}]}}`))
	if err != nil || cmd.Type != "shutdown" {
		t.Errorf("unknown field: %+v %v", cmd, err)
	}
}

func TestFrameReaderMalformedLines(t *testing.T) {
	// Blank, CR-only and whitespace-only lines are skipped; a NUL line is not
	// whitespace and is handed to the parser, which rejects it.
	reader := newFrameReader(strings.NewReader("\r\n\r\r\n \t \n\x00\n\xff\xfe\n{\"v\":1}\r\n\r"))
	for _, want := range []string{"\x00", "\xff\xfe", `{"v":1}`} {
		line, err := reader.next()
		if err != nil || string(line) != want {
			t.Fatalf("next() = %q, %v; want %q", line, err, want)
		}
		if want != `{"v":1}` {
			if _, perr := parseCommand(line); perr == nil {
				t.Errorf("%q parsed as a command", line)
			}
		}
	}
	if _, err := reader.next(); !errors.Is(err, io.EOF) {
		t.Fatalf("a trailing lone CR is not a frame: %v", err)
	}
	// Nothing but blank lines, or no bytes at all, is a clean EOF.
	for _, input := range []string{"", "\n", "\r\n\r\n", "   "} {
		if _, err := newFrameReader(strings.NewReader(input)).next(); !errors.Is(err, io.EOF) {
			t.Errorf("%q: %v, want EOF", input, err)
		}
	}
	// A CRLF-terminated frame at exactly the limit still passes (the CR is trimmed).
	exact := strings.Repeat("x", maxFrameBytes-1)
	line, err := newFrameReader(strings.NewReader(exact + "\r\n")).next()
	if err != nil || len(line) != maxFrameBytes-1 {
		t.Errorf("CRLF frame near the limit: len=%d err=%v", len(line), err)
	}
}

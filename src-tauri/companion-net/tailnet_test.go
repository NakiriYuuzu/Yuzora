package main

import (
	"io"
	"log"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// The real node's logs, and the std logger's, stay on disk: a login URL or a
// proxy URL with credentials must not reach the stderr the desktop logs.
func TestTSNetNodeLogsToItsStateDir(t *testing.T) {
	previous := log.Writer()
	t.Cleanup(func() { log.SetOutput(previous) })
	dir := filepath.Join(t.TempDir(), "net")
	node, err := newTSNetNode(dir, "yuzora")
	if err != nil {
		t.Fatal(err)
	}
	log.Printf("tshttpproxy: using proxy %q", "http://alice:s3cretPW@127.0.0.1:3128")
	if err := node.Close(); err != nil {
		t.Fatalf("closing a node that never started: %v", err)
	}
	logged, err := os.ReadFile(filepath.Join(dir, "tsnet.log"))
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(string(logged), "s3cretPW") {
		t.Errorf("the std logger did not reach tsnet.log: %q", logged)
	}
	if log.Writer() != io.Discard {
		t.Errorf("after Close the std logger writes to %T, not nowhere", log.Writer())
	}
}

package main

import (
	"context"
	"crypto/tls"
	"io"
	"log"
	"net"
	"os"
	"path/filepath"

	"tailscale.com/client/local"
	"tailscale.com/client/tailscale/apitype"
	"tailscale.com/ipn/ipnstate"
	"tailscale.com/tsnet"
)

// localAPI is the part of tailscale.com/client/local.Client the sidecar uses.
type localAPI interface {
	StatusWithoutPeers(ctx context.Context) (*ipnstate.Status, error)
	CertPair(ctx context.Context, domain string) (certPEM, keyPEM []byte, err error)
	GetCertificate(hi *tls.ClientHelloInfo) (*tls.Certificate, error)
	WhoIs(ctx context.Context, remoteAddr string) (*apitype.WhoIsResponse, error)
	Logout(ctx context.Context) error
}

// tailnet is the part of tsnet.Server the sidecar uses.
type tailnet interface {
	Start() error
	LocalClient() (localAPI, error)
	Listen(network, addr string) (net.Listener, error)
	Close() error
}

var _ localAPI = (*local.Client)(nil)

// tsnetNode is the real tailnet: an embedded Tailscale node whose own logs, and
// the std logger's, go to <dir>/tsnet.log, never to stderr, so a login URL or
// proxy credentials stay out of the desktop log. main turns off the upload of
// those logs to log.tailscale.com (setupProcess).
type tsnetNode struct {
	srv     *tsnet.Server
	logFile io.Closer
}

func newTSNetNode(dir, hostname string) (tailnet, error) {
	if err := os.MkdirAll(dir, 0o700); err != nil {
		return nil, err
	}
	logFile, err := os.OpenFile(filepath.Join(dir, "tsnet.log"), os.O_CREATE|os.O_WRONLY|os.O_TRUNC, 0o600)
	if err != nil {
		return nil, err
	}
	log.SetOutput(logFile)
	logf := log.New(logFile, "", log.LstdFlags|log.Lmicroseconds).Printf
	return &tsnetNode{
		srv:     &tsnet.Server{Dir: dir, Hostname: hostname, Ephemeral: false, Logf: logf, UserLogf: logf},
		logFile: logFile,
	}, nil
}

func (t *tsnetNode) Start() error { return t.srv.Start() }

func (t *tsnetNode) LocalClient() (localAPI, error) { return t.srv.LocalClient() }

func (t *tsnetNode) Listen(network, addr string) (net.Listener, error) {
	return t.srv.Listen(network, addr)
}

func (t *tsnetNode) Close() error {
	err := t.srv.Close()
	log.SetOutput(io.Discard)
	_ = t.logFile.Close()
	return err
}

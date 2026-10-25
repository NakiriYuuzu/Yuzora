"""Run against the preflighted companion-spike with w1:p1 already created.
No live Herdr calls; this program deliberately does not bootstrap sessions.
"""
from probe import *

try:
    a = spawn('control-a', SESSION + ['terminal', 'session', 'control', 'w1:p1', '--cols', '120', '--rows', '40'])
    time.sleep(1)
    b = spawn('observe-b', SESSION + ['terminal', 'session', 'observe', 'w1:p1', '--cols', '44', '--rows', '30'])
    time.sleep(1)
    send(a, {'type': 'terminal.input', 'text': "printf 'A_FRAME_MARKER_ABCDEFGHIJKLMNOPQRSTUVWXYZ_0123456789_END\\n'; stty size\r"})
    run('geometry-current', SESSION + ['api', 'snapshot'])
    send(b, {'type': 'terminal.input', 'text': 'echo OBSERVER_MUST_NOT_EXECUTE\r'})
    send(b, {'type': 'terminal.resize', 'cols': 20, 'rows': 10})
    run('pane-after-observer-input', SESSION + ['pane', 'read', 'w1:p1'])
    c = spawn('control-conflict', SESSION + ['terminal', 'session', 'control', 'w1:p1', '--cols', '120', '--rows', '40'])
    time.sleep(1)
    d = spawn('control-takeover', SESSION + ['terminal', 'session', 'control', 'w1:p1', '--cols', '120', '--rows', '40', '--takeover'])
    time.sleep(1)
    (FIX/'a-after-takeover.txt').write_text(f'exit={a.poll()}\n')
    send(d, {'type': 'terminal.input', 'text': 'stty size\r'})
    run('geometry-after-takeover', SESSION + ['api', 'snapshot'])
    send(d, {'type': 'terminal.release'})
    time.sleep(0.5)
    small = spawn('control-small', SESSION + ['terminal', 'session', 'control', 'w1:p1', '--cols', '44', '--rows', '30'])
    time.sleep(1)
    send(small, {'type': 'terminal.input', 'text': 'stty size\r'})
    run('geometry-small', SESSION + ['api', 'snapshot'])
    run('pane-small', SESSION + ['pane', 'read', 'w1:p1'])
    send(small, {'type': 'terminal.release'})
    time.sleep(0.5)
    # Two split panes make mobile's single-pane projection distinguishable.
    run('split', SESSION + ['pane', 'split', 'w1:p1', '--direction', 'right', '--no-focus'])
    run('geometry-before-full', SESSION + ['api', 'snapshot'])
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 30, 44, 0, 0))
    preflight()
    full = subprocess.Popen([WRAPPER, *SESSION, 'client'], stdin=slave, stdout=slave, stderr=slave,
                            env={**os.environ, 'TERM': 'xterm-256color'}, start_new_session=True)
    children.append(full)
    os.close(slave)
    transcript = bytearray()
    def drain(seconds):
        until = time.monotonic() + seconds
        while time.monotonic() < until:
            ready, _, _ = select.select([master], [], [], 0.1)
            if ready:
                try:
                    chunk = os.read(master, 65536)
                except OSError:
                    return
                transcript.extend(chunk)
    drain(2)
    run('geometry-full-before-input', SESSION + ['api', 'snapshot'])
    preflight()
    os.write(master, b'echo FULL_CLIENT_MARKER; stty size\r')
    drain(2)
    run('geometry-full-after-input', SESSION + ['api', 'snapshot'])
    run('pane-full', SESSION + ['pane', 'read', 'w1:p1'])
    (FIX/'full-client-44x30.ansi').write_bytes(transcript)
    preflight()
    os.write(master, b'\x02q')  # documented prefix+q detaches only this client
    drain(1)
    os.close(master)
    stop_child(full)
    # Observe/control should fail instead of implicitly restarting the session.
    run('session-stop', ['session', 'stop', 'companion-spike', '--json'])
    time.sleep(0.5)
    run('observe-stopped', SESSION + ['terminal', 'session', 'observe', 'w1:p1', '--cols', '44', '--rows', '30'])
    run('control-stopped', SESSION + ['terminal', 'session', 'control', 'w1:p1', '--cols', '44', '--rows', '30'])
    run('sessions-after-stopped-attempts', ['session', 'list', '--json'])
finally:
    for p in reversed(children):
        stop_child(p)

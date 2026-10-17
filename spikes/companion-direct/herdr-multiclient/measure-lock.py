"""Second isolated experiment: direct control resize lock vs 44-column full client."""
from probe import *

master = None
try:
    clean = preflight()
    assert all(s['name'] == 'default' and not s['running'] for s in clean['sessions'])
    config = T/'config/herdr/config.toml'
    config.parent.mkdir(parents=True, exist_ok=True)
    config.write_text('onboarding = false\n[terminal]\ndefault_shell = "/bin/sh"\nshell_mode = "non_login"\n[update]\nversion_check = false\nmanifest_check = false\n')
    server = spawn('lock-server', SESSION + ['server'])
    time.sleep(1)
    run('lock-workspace', SESSION + ['workspace', 'create', '--cwd', str(T/'home'), '--label', 'lock-spike', '--focus'])
    run('lock-split', SESSION + ['pane', 'split', 'w1:p1', '--direction', 'right', '--no-focus'])
    a = spawn('lock-control', SESSION + ['terminal', 'session', 'control', 'w1:p1', '--cols', '80', '--rows', '24'])
    time.sleep(0.5)
    send(a, {'type':'terminal.input', 'text':'stty size\r'})
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 30, 44, 0, 0))
    preflight()
    full = subprocess.Popen([WRAPPER, *SESSION, 'client'], stdin=slave, stdout=slave, stderr=slave,
        env={**os.environ, 'TERM':'xterm-256color'}, start_new_session=True)
    children.append(full)
    os.close(slave)
    transcript = bytearray()
    def drain(seconds):
        until=time.monotonic()+seconds
        while time.monotonic()<until:
            if select.select([master],[],[],0.1)[0]:
                try: transcript.extend(os.read(master,65536))
                except OSError: return
    drain(2)
    run('lock-full-before-input', SESSION+['api','snapshot'])
    preflight()
    os.write(master,b'echo LOCK_MARKER; stty size\r')
    drain(1)
    run('lock-full-after-input', SESSION+['api','snapshot'])
    run('lock-pane-after-input', SESSION+['pane','read','w1:p1'])
    # Full client geometry claims must not silently steal the direct control stream.
    send(a,{'type':'terminal.input','text':'echo CONTROL_STILL_WRITES; stty size\r'})
    drain(0.5)
    run('lock-pane-after-control', SESSION+['pane','read','w1:p1'])
    preflight()
    os.write(master,b'\x02q')
    drain(1)
    (FIX/'lock-full-client.ansi').write_bytes(transcript)
    os.close(master)
    master=None
    stop_child(full)
finally:
    if master is not None:
        os.close(master)
    run('lock-session-stop',['session','stop','companion-spike','--json'])
    for p in reversed(children):
        stop_child(p)
    run('lock-session-delete',['session','delete','companion-spike'])
    run('lock-sessions-after-delete',['session','list','--json'])

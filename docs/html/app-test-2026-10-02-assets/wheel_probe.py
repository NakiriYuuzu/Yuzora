import sys, tty, termios, select, json, time
from pathlib import Path
mode = sys.argv[1] if len(sys.argv) > 1 else 'alt-mouse'
log = Path(__file__).with_name(mode + '.jsonl')
old = termios.tcgetattr(sys.stdin)
count = 0
if mode == 'normal-mouse':
    for i in range(300): print(f'QA_HISTORY_{i:03d}')
else:
    sys.stdout.write('\x1b[?1049h')
if mode in ('alt-mouse', 'normal-mouse'):
    sys.stdout.write('\x1b[?1000h\x1b[?1006h')
else:
    sys.stdout.write('\x1b[?1007h')
def draw(data=b''):
    sys.stdout.write('\x1b[2J\x1b[H' + f'YUZORA WHEEL QA | mode={mode}\r\nEvents: {count}\r\nLast input: {data!r}\r\nScroll up/down in this terminal. Ctrl+C exits.\r\n')
    for i in range(5, 22): sys.stdout.write(f'Probe row {i:02d}\r\n')
    sys.stdout.flush()
try:
    tty.setraw(sys.stdin.fileno())
    draw()
    while True:
        if not select.select([sys.stdin], [], [], 120)[0]: break
        data = __import__('os').read(sys.stdin.fileno(), 4096)
        if b'\x03' in data or not data: break
        count += 1
        with log.open('a') as f: f.write(json.dumps({'time': time.time(), 'mode':mode, 'input':repr(data), 'hex':data.hex(), 'count':count})+'\n')
        draw(data)
finally:
    sys.stdout.write('\x1b[?1000l\x1b[?1006l\x1b[?1007l')
    if mode != 'normal-mouse': sys.stdout.write('\x1b[?1049l')
    sys.stdout.write('\r\nQA probe finished\r\n'); sys.stdout.flush()
    termios.tcsetattr(sys.stdin, termios.TCSADRAIN, old)

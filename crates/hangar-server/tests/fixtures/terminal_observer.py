#!/usr/bin/env python3
import os, sys, time
from pathlib import Path
root = Path(__file__).parent
root.joinpath('pid').write_text(str(os.getpid()))
with root.joinpath('spawns').open('a') as log:
    log.write(str(os.getpid()) + '\n')
name = sys.argv[-1].removeprefix('=')
mode = root.joinpath('mode').read_text() if root.joinpath('mode').exists() else 'normal'
def frame(n, body=''):
    sys.stdout.write(f'%begin 1 {n} 0\n' + body + f'%end 1 {n} 0\n')
    sys.stdout.flush()
frame(0)
for n, line in enumerate(sys.stdin, 1):
    with root.joinpath('commands').open('a') as log:
        log.write(line)
    commands = line.rstrip('\n').split(' ; ')
    wrapped = len(commands) == 3
    command = commands[1] if wrapped else commands[0]
    if not command.startswith(('display-message -p -t ', 'capture-pane -p ')):
        sys.exit(7)
    if wrapped and (not commands[0].removeprefix('display-message -p ').removeprefix('-l ').startswith('HG_START_') or not commands[2].removeprefix('display-message -p ').removeprefix('-l ').startswith('HG_END_')):
        sys.exit(8)
    start = commands[0].removeprefix('display-message -p ').removeprefix('-l ') + '\n'
    end = commands[2].removeprefix('display-message -p ').removeprefix('-l ') + '\n' if wrapped else ''
    if mode == 'legacy' and any(command.startswith('display-message -p -l ') for command in commands):
        sys.stdout.write(f'%begin 1 {n} 0\nunsupported option\n%error 1 {n} 0\n')
        sys.stdout.flush()
        continue
    if n == 3 and mode == 'late':
        body = '%begin 1 99 0\nstale\n\n\n\n%end 1 99 0\n'
        for chunk in [body[:7], body[7:21], body[21:]]:
            time.sleep(.01)
            sys.stdout.write(chunk)
            sys.stdout.flush()
        continue
    if wrapped:
        frame(n * 100, start)
    if mode == 'hold' and name == 'fixture' and n == 3:
        root.joinpath('blocked').write_text('yes')
        while not root.joinpath('resume').exists():
            time.sleep(.01)
    if mode == 'static-error':
        sys.stdout.write(f'%begin 1 {n * 100 + 17} 0\nfailed\n%error 1 {n * 100 + 17} 0\n')
        sys.stdout.flush()
        continue
    if n >= 3 and mode == 'timeout':
        time.sleep(30)
    if n >= 3 and mode == 'eof':
        sys.exit(0)
    if n >= 3 and mode == 'utf8':
        sys.stdout.buffer.write(f'%begin 1 {n * 100 + 17} 0\n'.encode() + b'\xff\n' + f'%end 1 {n * 100 + 17} 0\n'.encode())
        sys.stdout.buffer.flush()
        continue
    if n >= 3 and mode == 'error':
        sys.stdout.write(f'%begin 1 {n * 100 + 17} 0\nfailed\n%error 1 {n * 100 + 17} 0\n')
        sys.stdout.flush()
        continue
    if n == 3 and mode == 'prefill':
        frames = [(n * 100 + 17, '%3\tfixture\t20\t4\t0\t0\t0\n'), (78, 'stale\n\n\n\n'),
                  (79, 'stale\n\n\n\n'), (80, '%3\tfixture\t20\t4\t0\t0\t0\n')]
        sys.stdout.write(''.join(f'%begin 1 {index} 0\n' + body + f'%end 1 {index} 0\n' for index, body in frames))
        sys.stdout.flush()
        continue
    if n == 2:
        sys.stdout.write('%output %9 wrong-pane\n%output %3 \\033[6n\n')
    columns, rows = (1024, 512) if mode == 'dimensions' else ((21, 4) if mode == 'resize' and n == 6 else (20, 4))
    frame(n * 100 + 17, f'%3\t{name}\t{columns}\t{rows}\t0\t0\t0\n' if command.startswith('display-message') else 'ready\n\n\n\n')
    if wrapped:
        if n == 3 and mode == 'missing-end':
            continue
        if n == 3 and mode == 'late-end':
            time.sleep(.12)
            root.joinpath('end-sent').write_text('yes')
        frame(n * 100 + 39, 'wrong-marker\n' if n == 3 and mode == 'wrong-end' else end)

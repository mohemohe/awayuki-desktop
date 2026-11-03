import argparse
import http.server
import json
import os
from pathlib import Path
import re
import subprocess
import threading

source = Path(__file__).resolve().parent
parser = argparse.ArgumentParser(description='Bounded local WKWebView image decoder lifetime experiment')
parser.add_argument('mode', choices=['auto', 'sync', 'async'])
parser.add_argument('kind', choices=['animated', 'static'])
parser.add_argument('--output-root', type=Path, default=source.parents[2]/'build/image-decoder-investigation')
args = parser.parse_args()
root = args.output_root.resolve()
root.mkdir(parents=True, exist_ok=True)
mode, kind = args.mode, args.kind
for name, binary in [('DecoderProbe.swift', 'decoder-probe'), ('ComparePixels.swift', 'compare-pixels')]:
    subprocess.run(['swiftc', '-module-cache-path', str(root/'module-cache'), str(source/name), '-o', str(root/binary), '-framework', 'AppKit', '-framework', 'WebKit', '-framework', 'ImageIO'], check=True)
os.chdir(root)
output = root / f'{mode}-{kind}'
output.mkdir(exist_ok=True)
for suffix in ('a', 'b'):
    (root/f'{mode}-{kind}-animation-{suffix}.png').unlink(missing_ok=True)

class Handler(http.server.SimpleHTTPRequestHandler):
    def log_message(self, *args):
        pass
    def end_headers(self):
        self.send_header('Cache-Control', 'public, max-age=3600')
        super().end_headers()

def webcontents():
    result = subprocess.check_output(['ps', '-axo', 'pid=,comm='], text=True)
    return {int(line.split()[0]) for line in result.splitlines() if line.strip().endswith('/com.apple.WebKit.WebContent')}

server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Handler)
threading.Thread(target=server.serve_forever, daemon=True).start()
before = webcontents()
subprocess.run([str(root/'decoder-probe'), 'generate'], check=True)
with (output/'stderr.log').open('w') as stderr:
    process = subprocess.Popen([str(root/'decoder-probe'), mode, str(server.server_port), kind], stdout=subprocess.PIPE, stderr=stderr, text=True)
    timer = threading.Timer(95, process.kill)
    timer.start()
    samples = []
    pid = None
    try:
        for line in process.stdout:
            try:
                event = json.loads(line)
            except ValueError:
                continue
            stage = event['stage']
            if pid is None:
                fresh = webcontents() - before
                if len(fresh) == 1:
                    pid = fresh.pop()
                else:
                    event['pidCandidates'] = sorted(fresh)
            if pid and stage not in ('finished', 'timeout', 'animation-snapshot'):
                sample_file = output/f'{stage}.sample.txt'
                sample_file.unlink(missing_ok=True)
                subprocess.run(['sample', str(pid), '1', '10', '-file', str(sample_file)], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=8, check=True)
                sample = sample_file.read_text()
                event['webcontentPid'] = pid
                event['decoderQueues'] = len(re.findall(r'DispatchQueue_\d+: org.webkit.ImageDecoder', sample))
                event['hardLimitReached'] = 'Dispatch Thread Hard Limit' in sample
                footprint = re.search(r'^Physical footprint:\s+(.*)$', sample, re.M)
                event['footprint'] = footprint.group(1) if footprint else None
            samples.append(event)
            print(json.dumps(event), flush=True)
        code = process.wait(timeout=10)
        print(json.dumps({'exitCode':code}), flush=True)
        if code != 0 or pid is None:
            raise RuntimeError('fixture failed or its unique WebContent process could not be identified')
        stages = {event['stage']: event for event in samples}
        for stage in ['baseline', 'idle-start', 'idle-end'] + [f'{action}-{batch}' for batch in range(3) for action in ('loaded', 'removed')]:
            event = stages.get(stage, {})
            expected_images = 50 if stage.startswith('loaded-') else 0
            if (event.get('visibility') != 'visible' or event.get('dom') != expected_images
                    or event.get('loaded') != expected_images or 'decoderQueues' not in event):
                raise RuntimeError(f'incomplete or hidden fixture stage: {stage}')
        first = root/f'{mode}-{kind}-animation-a.png'
        second = root/f'{mode}-{kind}-animation-b.png'
        if not first.exists() or not second.exists():
            raise RuntimeError('native animation snapshots missing')
        comparison = subprocess.check_output([str(root/'compare-pixels'), str(first), str(second)], text=True)
        (root/f'{mode}-{kind}-animation-pixels.json').write_text(comparison)
        print(comparison.strip(), flush=True)
    finally:
        timer.cancel()
        if process.poll() is None:
            process.kill()
        (output/'results.json').write_text(json.dumps(samples, indent=2)+'\n')
        server.shutdown()

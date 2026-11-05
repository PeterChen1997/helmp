#!/usr/bin/env python3
"""并行分支合并辅助：冲突块两边取并集。

- Cargo.toml：同名依赖只保留一行，features 取并集，其余字段以先出现的为准。
- 其他文件（如 providers/mod.rs）：两边的行按出现顺序去重拼接。
"""
import re
import sys

CONFLICT = re.compile(r'<<<<<<< [^\n]*\n(.*?)=======\n(.*?)>>>>>>> [^\n]+\n', re.S)


def merge_deps(ours: str, theirs: str) -> str:
    order, lines = [], {}
    for line in (ours + theirs).splitlines():
        m = re.match(r'^([A-Za-z0-9_-]+)\s*=', line)
        if not m:
            if line.strip() and not line.startswith('#'):
                order.append(line)
            continue
        key = m.group(1)
        if key not in lines:
            order.append(key)
            lines[key] = line
            continue
        a, b = lines[key], line
        fa = re.search(r'features = \[(.*?)\]', a)
        fb = re.search(r'features = \[(.*?)\]', b)
        if fa and fb:
            feats = []
            for f in re.findall(r'"([^"]+)"', fa.group(1) + ',' + fb.group(1)):
                if f not in feats:
                    feats.append(f)
            lines[key] = a.replace(fa.group(0), 'features = [' + ', '.join(f'"{f}"' for f in feats) + ']')
    return '\n'.join(lines.get(k, k) for k in order) + '\n'


def merge_lines(ours: str, theirs: str) -> str:
    out = []
    for line in (ours + theirs).splitlines(keepends=True):
        if line not in out:
            out.append(line)
    return ''.join(out)


path = sys.argv[1]
text = open(path, encoding='utf-8').read()
fn = merge_deps if path.endswith('Cargo.toml') else merge_lines
text = CONFLICT.sub(lambda m: fn(m.group(1), m.group(2)), text)
open(path, 'w', encoding='utf-8').write(text)

#!/usr/bin/env python3
"""合并辅助：Cargo.toml 依赖冲突取并集（两边都保留）。仅用于并行开发分支合并。"""
import re, sys
p = sys.argv[1]
s = open(p, encoding='utf-8').read()
s = re.sub(r'<<<<<<< [^\n]*\n(.*?)=======\n(.*?)>>>>>>> [^\n]+\n', lambda m: m.group(1) + m.group(2), s, flags=re.S)
open(p, 'w', encoding='utf-8').write(s)

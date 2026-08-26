#!/usr/bin/env python3
"""Pawork UI fixture 敏感信息扫描（R1 Wave B）。

用法：
  python3 scripts/ui-fixture-scan.py [root ...]

不带参数时默认扫描仓内 fixtures/ui/；也可追加运行时 fixture root（例如
/tmp 下的隔离数据目录）一起扫描。命中任何规则立即以 exit 2 退出并逐条
打印发现；全部干净时 exit 0。路径不存在等使用错误 exit 1。

规则（brief §8 冻结，正则以本文件为准）：
  token-prefix     类 token 前缀（sk- / glpat- / xai- / ghp_ 等）后接
                   至少 8 位 token 字符；
  key-assignment   api[_-]?key / token / secret 后接 [:=] 再接
                   ≥20 位连续赋值字符；
  home-path        /Users/<name> 或 /home/<name> 形绝对路径（泄露本机
                   用户目录；seed.json 中路径只允许 ${ROOT} 占位）；
  seed-abs-path    seed.json 内出现任何绝对路径字符串（schema 要求全部
                   使用 ${ROOT} 占位符）；
  auth-json        扫描范围内出现名为 auth.json 的文件（fixture data
                   目录禁止真实凭证文件）；
  gui-token-shape  文本中 64 位 hex 且同一行前方 48 字符上下文含 token
                   字样（gui.token 值形状泄露；名为 gui.token 的合法
                   token store 文件本身豁免此条）；
  io-error         文件不可读（fail-closed，按命中处理）。

跳过目录：.git / target / node_modules；跳过 .DS_Store 与 >8MiB 的大文件
（以 UTF-8 errors=replace 解码后按行匹配，二进制内容同样参与规则匹配）。
仅读取 regular file；Unix socket、FIFO 与 symlink 等非 regular file 不读取，
但名为 auth.json 的非 regular 项仍按文件名规则拦截。
报告中的命中值一律掩码，不在输出中回显完整敏感串。
"""

from __future__ import annotations

import argparse
import os
import re
import stat
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Iterator, List, Optional, Tuple

TOKEN_PREFIXES = ("sk-", "glpat-", "xai-", "ghp_", "gho_", "github_pat_")
MIN_TOKEN_TAIL = 8
TOKEN_PREFIX_RE = re.compile(
    r"(?<![A-Za-z0-9])(?:(%s)[A-Za-z0-9_\-+/=.]{%d,})"
    % ("|".join(re.escape(p) for p in TOKEN_PREFIXES), MIN_TOKEN_TAIL)
)
KEY_ASSIGN_RE = re.compile(
    r"(?i)\b(api[_-]?key|token|secret)\b\s*[:=]\s*[\"']?([A-Za-z0-9_\-+/=.]{20,})"
)
HOME_PATH_RE = re.compile(r"/(?:Users|home)/[A-Za-z0-9._\-]+")
SEED_JSON_ABS_RE = re.compile(r"\"(/[^\"]*)\"")
HEX64_RE = re.compile(r"(?<![0-9A-Fa-f])[0-9A-Fa-f]{64}(?![0-9A-Fa-f])")
TOKEN_HINT_RE = re.compile(r"(?i)token")
TOKEN_CONTEXT_CHARS = 48
SKIP_DIRS = frozenset({".git", "target", "node_modules"})
SKIP_FILES = frozenset({".DS_Store"})
MAX_FILE_BYTES = 8 * 1024 * 1024


@dataclass(frozen=True)
class Finding:
    root: Path
    path: Path
    line: int
    rule: str
    detail: str


def mask(value: str, keep: int = 4) -> str:
    """掩码命中值：只保留前几位与长度，不回显完整敏感串。"""
    if len(value) <= keep:
        return "*" * len(value)
    return "%s…(len=%d)" % (value[:keep], len(value))


def iter_files(root: Path) -> Iterator[Tuple[Path, Path]]:
    """产出 (绝对路径, 相对 root 路径)。"""
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames[:] = sorted(d for d in dirnames if d not in SKIP_DIRS)
        for name in sorted(filenames):
            if name in SKIP_FILES:
                continue
            absolute = Path(dirpath) / name
            try:
                if not stat.S_ISREG(absolute.lstat().st_mode) and name != "auth.json":
                    continue
            except OSError:
                # 交给 scan_file 产出 fail-closed 的 io-error。
                pass
            yield absolute, absolute.relative_to(root)


def scan_file(root: Path, path: Path) -> List[Finding]:
    findings: List[Finding] = []
    rel = path.relative_to(root)

    def add(rule: str, line: int, detail: str) -> None:
        findings.append(Finding(root, rel, line, rule, detail))

    if path.name == "auth.json":
        add("auth-json", 0, "fixture 范围内禁止出现 auth.json（真实凭证文件）")

    try:
        file_stat = path.lstat()
        if not stat.S_ISREG(file_stat.st_mode):
            return findings
        if file_stat.st_size > MAX_FILE_BYTES:
            return findings
        raw = path.read_bytes()
    except OSError as error:
        add("io-error", 0, "无法读取：%s" % error)
        return findings
    text = raw.decode("utf-8", errors="replace")

    is_seed = path.name == "seed.json"
    # gui.token 是布局内合法的 token store 文件，其内容本身就是 64 hex；
    # 该文件豁免 gui-token-shape，其余规则照常。
    token_store = path.name == "gui.token"

    for lineno, line in enumerate(text.splitlines(), start=1):
        for match in TOKEN_PREFIX_RE.finditer(line):
            add(
                "token-prefix",
                lineno,
                "类 token 前缀 %s（完整值已掩码）" % mask(match.group(0), 3),
            )
        for match in KEY_ASSIGN_RE.finditer(line):
            add(
                "key-assignment",
                lineno,
                "%s 赋值形状 %s" % (match.group(1).lower(), mask(match.group(2))),
            )
        for match in HOME_PATH_RE.finditer(line):
            prefix = "/Users/" if match.group(0).startswith("/Users/") else "/home/"
            add(
                "home-path",
                lineno,
                "本机用户绝对路径 %s<redacted>" % prefix,
            )
        if is_seed:
            for match in SEED_JSON_ABS_RE.finditer(line):
                add(
                    "seed-abs-path",
                    lineno,
                    "seed.json 禁止绝对路径（只允许 ${ROOT} 占位）：%s" % match.group(1),
                )
        if not token_store:
            for match in HEX64_RE.finditer(line):
                context = line[max(0, match.start() - TOKEN_CONTEXT_CHARS) : match.start()]
                if TOKEN_HINT_RE.search(context):
                    add("gui-token-shape", lineno, "64 位 hex 且上下文含 token 字样 %s" % mask(match.group(0), 6))
    return findings


def scan_root(root: Path) -> Tuple[List[Finding], int]:
    root = root.resolve()
    if not root.is_dir():
        raise NotADirectoryError(root)
    findings: List[Finding] = []
    scanned = 0
    for absolute, _rel in iter_files(root):
        scanned += 1
        findings.extend(scan_file(root, absolute))
    return findings, scanned


def default_roots() -> List[Path]:
    return [Path(__file__).resolve().parent.parent / "fixtures" / "ui"]


def main(argv: Optional[List[str]] = None) -> int:
    parser = argparse.ArgumentParser(
        description="Pawork UI fixture 敏感信息扫描（命中 exit 2）"
    )
    parser.add_argument(
        "roots",
        nargs="*",
        help="要扫描的目录（默认：仓内 fixtures/ui/）",
    )
    args = parser.parse_args(argv)
    roots = [Path(r) for r in args.roots] or default_roots()

    all_findings: List[Finding] = []
    total = 0
    for root in roots:
        try:
            findings, scanned = scan_root(root)
        except NotADirectoryError:
            print("ui-fixture-scan: 路径不存在或不是目录：%s" % root, file=sys.stderr)
            return 1
        all_findings.extend(findings)
        total += scanned
        print("扫描 %s：%d 个文件，%d 处命中" % (root, scanned, len(findings)))

    for finding in all_findings:
        location = "%s:%s" % (finding.path, finding.line if finding.line else "")
        print("HIT [%s] %s %s" % (finding.rule, location, finding.detail))
    if all_findings:
        print("敏感信息扫描失败：%d 处命中（exit 2）" % len(all_findings))
        return 2
    print("敏感信息扫描通过：%d 个文件（exit 0）" % total)
    return 0


if __name__ == "__main__":
    sys.exit(main())

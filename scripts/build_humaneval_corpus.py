#!/usr/bin/env python3
"""Generate the bundled coding corpus from HumanEval (first 100 tasks).

No third-party dependencies. Either pass a local HumanEval.jsonl[.gz] path, or
let it download the canonical file from the openai/human-eval repository.

Usage:
  python scripts/build_humaneval_corpus.py                    # downloads
  python scripts/build_humaneval_corpus.py /path/HumanEval.jsonl

Writes crates/sigo-core/assets/humaneval_sample.jsonl (100 lines) in the
{task_id, category, prompt, entry_point, test} schema the Rust loader expects.
"""
import json
import sys
import gzip
import pathlib
import urllib.request

URL = "https://raw.githubusercontent.com/openai/human-eval/master/data/HumanEval.jsonl.gz"


def read_rows():
    if len(sys.argv) > 1:
        raw = open(sys.argv[1], "rb").read()
    else:
        raw = urllib.request.urlopen(URL, timeout=30).read()
    if raw[:2] == b"\x1f\x8b":  # gzip magic number
        raw = gzip.decompress(raw)
    return [json.loads(line) for line in raw.decode("utf-8").splitlines() if line.strip()]


def main():
    rows = read_rows()[:100]
    out = pathlib.Path(__file__).resolve().parent.parent / "crates/sigo-core/assets/humaneval_sample.jsonl"
    out.parent.mkdir(parents=True, exist_ok=True)
    with out.open("w", encoding="utf-8") as f:
        for r in rows:
            f.write(json.dumps({
                "task_id": r["task_id"],
                "category": "coding-verifiable",
                "prompt": r["prompt"],
                "entry_point": r["entry_point"],
                "test": r["test"],
            }, ensure_ascii=False) + "\n")
    print(f"wrote {len(rows)} tasks to {out}")


if __name__ == "__main__":
    main()

#!/usr/bin/env python3
"""Capture a held-out Japanese compiler corpus. Requires TypeScript 5.9.3.

Install the pinned compiler separately; pass its lib/tsc.js path. Gold pages
and snippets are fixed here before retrieval, never inferred from search hits.
"""
import argparse
import hashlib
import json
import re
import subprocess
import tempfile
from datetime import datetime, timezone
from pathlib import Path

DOC = "https://www.typescriptlang.org/docs/handbook/2/"
# source, topic pages, independent shortened query terms
CASES = [
    ("const total = missingInvoiceTotal;", ["everyday-types.html"], ["TypeScript", "名前", "missingInvoiceTotal"]),
    ('const label: string = 42;', ["everyday-types.html"], ["TypeScript", "string", "number", "割り当て"]),
    ('const receipt = { sum: 12 }; receipt.currency;', ["objects.html"], ["TypeScript", "プロパティ", "currency"]),
    ('const receipt: {readonly sum: number} = {sum: 12}; receipt.sum = 13;', ["objects.html"], ["TypeScript", "読み取り専用", "sum"]),
    ('const tax = 10; tax = 11;', ["everyday-types.html"], ["TypeScript", "定数", "割り当て"]),
    ('function invoice(item) { return item; }', ["functions.html"], ["TypeScript", "パラメーター", "any"]),
    ('function ship(id: number) {} ship("parcel");', ["functions.html"], ["TypeScript", "引数", "number", "string"]),
    ('function ship(id: number, city: string) {} ship(12);', ["functions.html"], ["TypeScript", "引数", "必要"]),
    ('const receipt: {sum: number; currency: string} = {sum: 12};', ["objects.html"], ["TypeScript", "プロパティ", "currency", "ありません"]),
    ('const receipt = {sum: 12}; const key: string = "sum"; receipt[key];', ["objects.html"], ["TypeScript", "インデックス", "string", "any"]),
    ('const receipt: {sum: number} = {sum: 12, rebate: 2};', ["objects.html"], ["TypeScript", "オブジェクト", "既知", "プロパティ"]),
    ('let parcel = 1; let parcel = 2;', ["everyday-types.html"], ["TypeScript", "ブロック", "再宣言"]),
    ('function deliver() { await Promise.resolve(1); }', ["functions.html"], ["TypeScript", "await", "async"]),
    ('abstract class Parcel { abstract send(): void; } new Parcel();', ["classes.html"], ["TypeScript", "抽象", "インスタンス"]),
    ('class Parcel { private tracking = 1; } new Parcel().tracking;', ["classes.html"], ["TypeScript", "private", "アクセス"]),
    ('class Parcel { protected tracking = 1; } new Parcel().tracking;', ["classes.html"], ["TypeScript", "protected", "アクセス"]),
    ('function send(parcel?: {id: number}) { return parcel.id; }', ["narrowing.html"], ["TypeScript", "undefined", "可能性"]),
    ('function send(parcel: {id: number} | null) { return parcel.id; }', ["narrowing.html"], ["TypeScript", "null", "可能性"]),
    ('function send(parcel: unknown) { return parcel.id; }', ["narrowing.html"], ["TypeScript", "unknown", "型"]),
    ('const shipment = 12; shipment();', ["functions.html"], ["TypeScript", "呼び出す", "シグネチャ"]),
    ('const shipment = 12; new shipment();', ["functions.html"], ["TypeScript", "構築", "シグネチャ"]),
    ('function send(parcel: string): number { if (parcel) return 1; }', ["functions.html"], ["TypeScript", "return", "undefined"]),
    ('function send(parcel: "small") { if (parcel === "large") return 1; }', ["narrowing.html"], ["TypeScript", "比較", "重複"]),
    ('const [parcel] = 12;', ["objects.html"], ["TypeScript", "Symbol.iterator", "反復"]),
    ('const key = {}; const parcel = {[key]: 1};', ["objects.html"], ["TypeScript", "計算", "プロパティ", "string"]),
    ('type Parcel = string | number; interface Package extends Parcel {}', ["objects.html"], ["TypeScript", "インターフェイス", "拡張"]),
    ('function send(): string { const parcel = 1; }', ["functions.html"], ["TypeScript", "return", "戻り値"]),
    ('function send(parcel: string): string; function send(parcel: number): number {return parcel;}', ["functions.html"], ["TypeScript", "オーバーロード", "互換"]),
    ('import parcel from "missing-parcel-package-unique";', ["modules.html"], ["TypeScript", "モジュール", "型宣言", "見つかりません"]),
    ('function send(id: number, city: string) {} const args = [1, "Tokyo"]; send(...args);', ["functions.html"], ["TypeScript", "spread", "タプル"]),
]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--compiler", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    version = subprocess.check_output(["node", str(args.compiler), "--version"], text=True).strip()
    if version != "Version 5.9.3":
        parser.error("This corpus pins TypeScript 5.9.3")
    flags = ["--pretty", "false", "--locale", "ja", "--strict", "--noEmit", "--target", "ES2022", "--module", "commonjs"]
    with tempfile.TemporaryDirectory(prefix="rustxng-compiler-") as directory:
        directory = Path(directory)
        files = []
        for i, (source, _, _) in enumerate(CASES, 1):
            filename = f"case-{i:02}.ts"
            (directory / filename).write_text("export {};\n" + source + "\n")
            files.append(filename)
        run = subprocess.run(["node", str(args.compiler.resolve()), *flags, *files], cwd=directory, capture_output=True, text=True)
    messages = {}
    for line in run.stdout.splitlines():
        match = re.match(r"(case-\d+\.ts)\((\d+),(\d+)\): error TS(\d+): (.*)", line)
        if match:
            messages.setdefault(match[1], []).append({"line": int(match[2]), "column": int(match[3]), "code": match[4], "message": match[5]})
    cases = []
    for i, (source, pages, keywords) in enumerate(CASES, 1):
        captured = messages.get(f"case-{i:02}.ts", [])
        if not captured:
            raise RuntimeError(f"Case {i} did not produce a compiler error")
        error = captured[0]
        cases.append({"id": f"compiler-ja-{i:02}", "kind": "pinpoint", "typescript_version": "5.9.3", "source": "export {};\n" + source + "\n", "diagnostic_code": error["code"], "exact_error": f'TS{error["code"]}: {error["message"]}', "captured_diagnostics": captured, "expected_reference_urls": [DOC + page for page in pages], "keyword_terms": keywords, "source_url": "https://github.com/microsoft/TypeScript/tree/v5.9.3", "source_license": "Apache-2.0", "case_origin": "independently executed compiler snippet; topic references fixed before retrieval; no gold-page indexing or ranking tuning"})
    corpus = {"schema_version": 1, "captured_at": datetime.now(timezone.utc).isoformat(), "dataset_license": "CC0-1.0 for original snippets; compiler diagnostic text Apache-2.0", "compiler": {"version": version, "distribution": "https://registry.npmjs.org/typescript/-/typescript-5.9.3.tgz", "runtime_sha256": {str(part.relative_to(args.compiler.parent)): hashlib.sha256(part.read_bytes()).hexdigest() for part in [args.compiler, args.compiler.parent / "_tsc.js", args.compiler.parent / "ja/diagnosticMessages.generated.json"]}, "flags": flags}, "compiler_transcript": run.stdout.splitlines(), "cases": cases}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(corpus, ensure_ascii=False, indent=2) + "\n")
    print(f"Captured {len(cases)} compiler cases; compiler exit {run.returncode}")


if __name__ == "__main__":
    main()

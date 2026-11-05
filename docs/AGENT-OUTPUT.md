# MCP output budgets

Both tools accept `max_payload_chars` (default 12000) and `max_payload_bytes` (default 24000).
An exact token budget requires `tokenizer` and `max_payload_tokens` together.

```json
{"query":"Rust 所有権","limit":10,"tokenizer":"cl100k_base","max_payload_tokens":4096}
```

The supported encodings are `cl100k_base` and `o200k_base`, implemented by Rust `tiktoken 4.1.2`.
Their vocabularies are embedded in the binary. Token limits range from 1 to 64000.
Special-token-looking text is counted as ordinary text. The response echoes the selected
encoding and limit in `token_budget`.

| Counting scope | Representation |
| --- | --- |
| Included | Compact JSON serialization of MCP `structuredContent`, with sorted object keys |
| Included | Complete URLs, titles, snippets or page text, passages, pagination, diagnostics and budget metadata |
| Outside this count | JSON-RPC envelope, MCP text-content copy, client SDK formatting, caller prompt and model framing |

Character, byte and token limits apply together. JSON escapes contribute to the serialized limits.
UTF-8 bytes and Unicode character offsets have separate units.

`search.limit` requests 1–10 results, default 5. Snippets contain at most 500 characters.
Packing first shortens trailing snippets and then omits trailing results as required.
`next_offset` continues within the collected first 200 ranked results.
Complete source URLs and available author, date and licence metadata are preserved for retained results.
Missing source fields remain absent.

`read_page.max_chars` requests 1–20000 Unicode characters, default 4000.
`offset` and `next_offset` count Unicode characters. Text extraction is capped at 200000 characters.
An optional `query` selects up to three relevant passages from the extracted page.
Packing attempts to retain passages while shortening the text slice, then omits passages if required.
`source_truncated` reports extraction limits and `truncated` also reports output omission.
A budget too small for the required URL and metadata returns an explicit tool error.

Only URLs actually delivered by `search` become readable in the same MCP session.
Page retrieval uses public-address DNS and redirect validation, robots policy, bounded bodies and concurrency.
Explicit page-language preferences are documented with the fetch settings.
The output contains external content and carries an untrusted-content notice.

The frozen 100-response comparison retained all 1000 source URLs and the same hit@1/@5/MRR.
Compact output reduced median bytes from 9978 to 6433 and cl100k tokens from 2429 to 1542.
With a 4096-token limit, the leading 80 snippet characters survived in 997/1000 cl100k results
and 1000/1000 o200k results. Steady-state packing p95 was 0.105/0.111 ms on the recorded machine.
These measurements cover serialization and source retention. The retained observations and
counting protocol are in the measurement report. [Measurements](ISSUE-RESOLUTION-2026-11-05.md)

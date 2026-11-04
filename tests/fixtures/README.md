# PDF fixture

`japanese.pdf` is an original, hand-built one-page PDF with the text
`日本語の政策資料 本文抽出を検証します。`.
It uses `UniJIS-UCS2-H` and `Adobe-Japan1` without a `ToUnicode` map,
so the test requires the Japanese CMap data distributed with Poppler.
The file contains no external content or embedded font program.

`empty.pdf` is an original one-page PDF without a text layer.

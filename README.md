# document-packet

PDF 原本を変更せず、検査・ページ画像化・画像/テキストの overlay・完成物検証を再現可能な JSON manifest で行う Rust CLI。

## Commands

```sh
document-packet inspect --pdf input.pdf --json
document-packet render --pdf input.pdf --out-dir .claude-dev/task/pages --dpi 144
document-packet apply --manifest .claude-dev/task/document-packet.json
document-packet verify --manifest .claude-dev/task/document-packet.json
```

`apply` は既存 PDF の page tree と内容を保ち、指定ページへ透明 PNG/JPEG と Helvetica の ASCII text を追加する。source/output が同じ manifest は拒否する。完了時に source hash が変わっていないこと、page count・MediaBox・rotation、各 operation marker/resource、サイズ条件を `verify` する。`render` は Ghostscript で全ページを PNG 化する。

## Manifest

パスは manifest 所在ディレクトリ基準。PDF 座標は左下原点・point 単位。

```json
{
  "schema": 1,
  "source": "../../projects/example/contract.pdf",
  "output": "../../projects/example/contract-signed.pdf",
  "expected_source_sha256": "<document-packet inspect の値>",
  "expected_page_count": 12,
  "min_output_bytes": 1000,
  "max_output_bytes": 20000000,
  "operations": [
    {
      "type": "image",
      "page": 12,
      "path": "signature.png",
      "x": 344,
      "y": 215,
      "width": 180,
      "height": 33.88
    },
    {
      "type": "text",
      "page": 12,
      "text": "Date: 2026/08/28",
      "x": 347,
      "y": 196,
      "size": 9
    }
  ]
}
```

署名画像を使うこと自体の承認はこのツールの外で行う。manifest と完成 PDF をユーザーが確認してから外部送信する。

## Development

```sh
nix develop -c cargo fmt --check
nix develop -c cargo clippy --all-targets -- -D warnings
nix develop -c cargo test
```

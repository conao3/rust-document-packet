# document-packet

PDF 原本を変更せず、検査・ページ画像化・画像/テキストの overlay・完成物検証を再現可能な JSON manifest で行う Rust CLI。

## Commands

入力は AWS CLI と同じ文法で受ける: `document-packet [global options] <command> <subcommand> [--parameter value ...]`。

```sh
document-packet pdf describe-pdf --pdf-file input.pdf
document-packet pdf render-pages --pdf-file input.pdf --output-dir .claude-dev/task/pages --dpi 144
document-packet manifest apply-manifest --manifest-file .claude-dev/task/document-packet.json
document-packet manifest verify-manifest --manifest-file .claude-dev/task/document-packet.json
```

| command | subcommand | parameters | 出力 |
|---|---|---|---|
| `pdf` | `describe-pdf` | `--pdf-file` | `PdfFile`, `Sha256`, `Bytes`, `PageCount`, `Pages[].{Page, WidthPoints, HeightPoints, Rotation}` |
| `pdf` | `render-pages` | `--pdf-file`, `--output-dir`, `--prefix` (既定 `page`), `--dpi` (既定 144、72〜600) | `OutputDir`, `PageCount`, `Pages[].{Page, ImageFile}` |
| `manifest` | `apply-manifest` | `--manifest-file` | `SourceFile`, `SourceSha256`, `OutputFile`, `OutputSha256`, `OutputBytes`, `PageCount`, `OperationCount` |
| `manifest` | `verify-manifest` | `--manifest-file` | `apply-manifest` と同じ |

`apply-manifest` は既存 PDF の page tree と内容を保ち、指定ページへ透明 PNG/JPEG と Helvetica の ASCII text を追加する。source/output が同じ manifest は拒否する。完了時に source hash が変わっていないこと、page count・MediaBox・rotation、各 operation marker/resource、サイズ条件を `verify-manifest` と同じ手順で検査する。`render-pages` は Poppler (`pdftoppm`) で全ページを `<prefix>-NNN.png` に PNG 化する。

### 共通の文法

- グローバルオプション: `--output json|text|table` (既定 `json`)、`--debug`、`--version`。どの位置に置いてもよい。
- `help` は各階層で使える: `document-packet help`、`document-packet pdf help`、`document-packet pdf describe-pdf help`。
- パラメータはすべて `--kebab-case` の名前付き。値の先頭が `file://` なら、そのファイルの中身 (末尾の改行 1 つを除く) を値として読む。
- 出力のキーは PascalCase。値は `document-packet pdf describe-pdf --pdf-file input.pdf | jq -r '.Pages[0].WidthPoints'` のように取り出す。
- 終了コード: 0 成功、252 コマンドの文法・パラメータの誤り、254 処理対象側の誤り (PDF・manifest が読めない、hash・ページ構造・サイズの不一致、`pdftoppm` の失敗)、255 それ以外 (出力の書き込み失敗、`pdftoppm` が起動できない等)。

## Manifest

manifest はファイル形式で、キーは snake_case。パスは manifest 所在ディレクトリ基準。PDF 座標は左下原点・point 単位。

```json
{
  "schema": 1,
  "source": "../../projects/example/contract.pdf",
  "output": "../../projects/example/contract-signed.pdf",
  "expected_source_sha256": "<document-packet pdf describe-pdf の Sha256>",
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

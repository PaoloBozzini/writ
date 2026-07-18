# Writ — VS Code extension

Syntax highlighting + file icons for `.writ`.

## Layout

- `syntaxes/writ.tmLanguage.json` — TextMate grammar (highlighting).
- `language-configuration.json` — comments, brackets, auto-close.
- `icons/writ-file.svg` — file glyph.
- `fileicons/writ-icon-theme.json` — maps `.writ` → glyph.
- `package.json` — `contributes` wires it all up.

## Try it live

1. Open this folder in VS Code.
2. Press `F5` → launches an Extension Development Host.
3. Open any `.writ` file; highlighting is active.
4. For icons: `Preferences: File Icon Theme` → **Writ Icons**.

## Install locally

```bash
npm i -g @vscode/vsce
cd editors/vscode
vsce package        # -> writ-lang-0.1.0.vsix
code --install-extension writ-lang-0.1.0.vsix
```

## Grammar scope map

| Writ | scope |
|------|-------|
| `fn let mut type import export` | `keyword.declaration` |
| `if else match return` | `keyword.control` |
| `uses requires ensures` | `keyword.other.contract` |
| `true false` | `constant.language.boolean` |
| `"..."` | `string.quoted.double` |
| integers | `constant.numeric.integer` |
| `Int`, `Text`, `Tainted` (Capitalized) | `entity.name.type` |
| `// ...` | `comment.line` |

# Changelog

## 1.0.0 — 2026-11-21

Primeira versão pública. / First public release.

- Bolha flutuante (arrasta, gruda na borda, some em jogo em tela cheia) e ícone na bandeja.
- Conversa com uma sessão contínua do Claude Code: arquivos, PowerShell, web, Chrome, conectores e agentes, com cartões Permitir/Negar.
- Memória longa compactada automaticamente; histórico completo em Markdown.
- Lembretes em linguagem natural e verificações de hora em hora (9h–22h).
- Sonnet como modelo padrão, Opus sob demanda; Haiku nas tarefas internas.
- Economia de quota: detector de mudança de assunto, limite de contexto (~110 mil tokens), recomeço após 60 min parado e chamadas internas sem conectores MCP (~2 mil tokens em vez de ~216 mil).
- Instalador de um comando (`install.ps1`) e desinstalador.
- `config.json` e `prompt.md` editáveis em `%APPDATA%\Claudinho`.

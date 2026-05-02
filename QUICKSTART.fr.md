# Démarrage rapide

[English](QUICKSTART.md) · 📖 Français

5 minutes pour passer de zéro à un workspace indexé par Standardoc, requêtable
par les agents IA.

---

## 1. Installer le CLI

```sh
cargo install standardoc-cli
stdoc --version
```

Vous avez maintenant un binaire unique `stdoc` avec ses sous-commandes :

```sh
stdoc lsp <workspace>             # daemon writer principal (acquiert le fs lock)
stdoc mcp <workspace> --readonly  # daemon MCP en lecture seule
stdoc index <workspace>           # scan + index ponctuel
stdoc rescan <workspace>          # rebuild from scratch
stdoc purge-excluded <workspace>  # supprime les symboles matchant .stdignore
```

Vous pouvez vous arrêter ici si vous ne voulez que l'usage CLI / MCP standalone
avec Claude Desktop, Cursor ou le CLI Claude Code — voir [§5](#5-utiliser-sans-lextension-vscode).
Pour le flow VSCode intégré, continuez.

---

## 2. Installer l'extension VSCode

Cherchez **Standardoc** dans le panneau Extensions VSCode, ou récupérez le
dernier VSIX depuis les [releases](https://github.com/miralabs-tech/standardoc/releases) puis :

```sh
code --install-extension standardoc-X.Y.Z.vsix
```

L'extension auto-spawn le daemon, supervise les redémarrages, enregistre
Standardoc comme MCP server pour Copilot Chat / Claude Code dans VSCode, et
expose un item dans la status bar.

---

## 3. Initialiser un workspace

Ouvrez n'importe quel projet Rust ou TypeScript dans VSCode. À la première
activation, une notification apparaît :

> **Standardoc: Initialize this workspace?** (DB index + register MCP for Claude Code CLI)
>
> [Initialize] [Skip] [Never for this workspace] [Never (any workspace)]

Cliquez **Initialize**. L'extension :

1. Crée `.standardoc/` (index SQLite + métadonnées workspace)
2. Écrit `.mcp.json` à la racine du workspace avec des chemins absolus
3. Génère `.claude/skills/standardoc/SKILL.md` pour Claude Code
4. Spawn le daemon LSP (cold start ~5-15s à la première exécution)

Le cold start indexe chaque fichier `.rs` / `.ts` / `.tsx` / `.js` / `.jsx`.
Ensuite, un watcher garde l'index live.

> Pensez à ajouter `.mcp.json` à votre `.gitignore` si vous collaborez —
> le fichier contient des chemins absolus machine non portables.

---

## 4. Utilisation

### Depuis les agents IA (Claude Code / Copilot Chat dans VSCode)

Le skill se charge automatiquement. Posez juste à l'agent des questions
normales sur votre codebase :

> *« Où est `parse_workspace` défini ? Qui l'appelle ? »*

L'agent utilise `find_symbol` + `get_context` au lieu de grepper. ~100 tokens
par question au lieu de 30k.

### Depuis la palette de commandes VSCode

- `Standardoc: Find symbol` — InputBox + QuickPick sur `find_symbol`, ouvre le
  symbole choisi à sa source.
- `Standardoc: Get context for symbol at cursor` — lance `find_symbol` sur le
  mot sous le curseur, prend le top match, render `get_context(fqdn,
  depth=1)` dans l'output channel Standardoc.
- `Standardoc: Daemon: Stop` / `Start` / `Restart`
- `Standardoc: Refresh .mcp.json paths` — re-merge avec les chemins absolus
  courants après déplacement du workspace ou rebuild du binaire ailleurs.
- `Standardoc: Regenerate AI agent skill` — overwrite le SKILL.md template.

---

## 5. Utiliser sans l'extension VSCode

Standardoc MCP marche avec n'importe quel client MCP-aware. Ajoutez ceci à la
config MCP de votre client (remplacez `<workspace>` et `<binary>` par des
chemins absolus) :

```json
{
  "mcpServers": {
    "standardoc": {
      "type": "stdio",
      "command": "<binary>",
      "args": ["mcp", "<workspace>", "--readonly"]
    }
  }
}
```

Vous avez aussi besoin d'un daemon LSP actif (le **writer principal**) pour
garder l'index frais. Lancez-le dans un terminal :

```sh
stdoc lsp /chemin/abs/vers/workspace
```

Le LSP tient le fs lock du workspace ; plusieurs clients MCP `--readonly`
peuvent attacher en concurrence sur le même index SQLite sans contention.

Emplacements de config courants :

- **Claude Desktop** — `claude_desktop_config.json` (Settings → Developer → Edit Config)
- **Claude Code CLI** — `~/.claude.json` ou `.mcp.json` par projet
- **Cursor** — `~/.cursor/mcp.json` ou `.cursor/mcp.json` workspace

---

## 6. Régler l'index

### `.stdignore`

Auto-seedé à la racine du workspace à la première init. Syntaxe gitignore.
Le template par défaut exclut `.git/`, `node_modules/`, `target/`, `dist/`,
`build/`, `.old/`, `*-old/`, `test-export/`. Éditez librement — les ajouts
excluent des chemins, les retraits déclenchent un re-index automatique du
sous-arbre concerné.

### Pause / purge

- `stdoc purge-excluded <workspace>` retire de l'index tout symbole dont le
  fichier source matche désormais `.stdignore` (utile après enrichissement
  du fichier).

---

## La suite

- [README.fr.md](README.fr.md) — surface complète des features et schéma d'architecture
- [ABOUT.fr.md](ABOUT.fr.md) — pourquoi Standardoc existe et en quoi il se
  démarque de LSP / Sourcegraph / TypeDoc / etc.
- [FAQ.fr.md](FAQ.fr.md) — questions courantes
- [COMPARISON.fr.md](COMPARISON.fr.md) — comparaison avec les outils adjacents

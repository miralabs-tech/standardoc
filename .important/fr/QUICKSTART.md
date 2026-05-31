# Démarrage rapide

[English](../en/QUICKSTART.md) · 📖 Français

[Philosophie](storytelling/philosophy.md) · [Vision court terme](storytelling/vision-court-terme.md) · [Remarques](storytelling/remarques.md) · [FAQ](FAQ.md) · [Comparaison](COMPARISON.md) · [Support](SUPPORT.md)

5 minutes pour passer de zéro à un workspace indexé par
Standardoc, requêtable par les agents IA.

---

## 1. Installer l'extension VSCode

Cherche **Standardoc** dans le marketplace VSCode / Open VSX, ou
récupère le VSIX depuis
[releases](https://github.com/miralabs-tech/standardoc/releases) :

```sh
code --install-extension standardoc-X.Y.Z.vsix
```

> Tu veux juste le CLI sans VSCode ? Saute directement à
> [§5 — CLI standalone](#5-sans-lextension-vscode-cli-standalone).

---

## 2. Télécharger le binaire `standardoc`

À la première activation, l'extension affiche un toast :
**Standardoc needs to download the native binary for this platform** —
[Download] / [Later] / [Show logs]. **Download** fetch le `version.json`
épinglé, récupère l'archive plateforme, vérifie son SHA256, et installe
le binaire ; **Later** laisse une affordance `$(cloud-download)` dans la
status bar pour réessayer. (Le binaire ship séparément du VSIX pour
pouvoir se mettre à jour à son propre rythme.)

Une fois en place, l'extension supervise le daemon et enregistre
Standardoc comme MCP server pour Copilot Chat / Claude Code dans VSCode.

> *Dev / pre-release :* mets `standardoc.binaryPath` sur un chemin absolu
> (ex. `target/debug/standardoc`) — il prime toujours sur le binaire
> auto-téléchargé.

---

## 3. Initialiser un workspace

Ouvre un projet (Rust / TypeScript / JavaScript / React (JSX & TSX) / Vue / Svelte / Lua).
Notification 4 boutons à la première activation :

> **Standardoc: Initialize this workspace?**
> [Initialize] [Skip] [Never for this workspace] [Never (any workspace)]

Click **Initialize**. L'extension :

1. Crée `.standardoc/` — index SQLite + métadonnées
2. Écrit `.mcp.json` à la racine (cross-client merge, préserve
   les fields user existants)
3. Génère `.claude/skills/standardoc/SKILL.md` (enseigne MCP-first,
   protocole 3-phase, edge kinds, workflows recommandés)
4. Spawn le LSP daemon (primary writer) + le MCP daemon HTTP/SSE, puis
   cold start indexe ton workspace (5–15s suivant la taille, progrès
   visible via `$/progress` côté LSP)

Le watcher garde l'index live après le cold start. Toute
modification de `*.rs` / `*.ts` / `*.tsx` / `*.js` / `*.jsx` /
`*.vue` / `*.svelte` / `*.lua` déclenche un re-index incrémental.

Un **item Standardoc apparaît dans le status bar** (coin droit
de la fenêtre VSCode) — il affiche l'état du daemon en temps
réel et sert de point d'entrée à toutes les actions courantes.

> Pense à ajouter `.mcp.json` à ton `.gitignore` si tu collabores —
> le fichier contient des chemins absolus machine non portables.

---

## 4. Utiliser

### Avec un agent IA

Claude Code, Cursor, Continue, Copilot Chat, Aider, Goose, Cody,
n'importe quel client MCP-aware. Pose des questions normales sur
ta codebase. L'agent lit le skill auto-généré au boot et bascule
en mode MCP-first :

> *« Où est `parse_workspace` défini ? Qui l'appelle ? Quels
> enrichissements lui sont attachés ? »*

L'agent utilise `find_symbol` + `get_context` (depth=1 d'abord,
depth=2 ensuite) au lieu de grep. **~100 tokens par question au
lieu de 30k.**

### Hooks Claude Code installés automatiquement

L'init opt-in flow installe le **MCP-first guardrail** dans
`.claude/settings.json` du workspace — il empêche l'agent de
dégénérer en grep-loop. Trois hooks coordonnés :

- `SessionStart` *(reset)* — wipe le sentinel à chaque nouveau
  chat pour rester strict
- `PreToolUse` *(mark)* — pose le sentinel dès qu'un tool
  Standardoc est appelé
- `PreToolUse` *(check)* — bloque `Bash` / `Read` / `Grep` /
  `Glob` quand le sentinel est absent (seulement quand l'appel
  vise un chemin DANS le workspace — les lectures hors workspace,
  ex. `~/.claude`, ne sont jamais gatées)

Si tu utilises un autre client MCP-aware, configure tes propres
hooks équivalents :

```sh
standardoc claude pre-tool-hook --mode mark   # poser le sentinel
standardoc claude pre-tool-hook --mode check  # bloquer Bash/Grep/...
standardoc claude pre-tool-hook --mode reset  # wipe au SessionStart
```

### Le menu du status bar (clic en bas à droite)

L'item Standardoc dans le status bar ouvre un QuickPick avec
**les actions courantes** :

- **▶ Start daemon** / **■ Stop daemon** / **↻ Restart daemon**
- **🗑 Purge excluded paths** — purge les symboles dont le
  fichier source matche désormais `.stdignore`

### Palette de commandes VSCode (`Ctrl+Shift+P`)

Toutes les actions du status bar sont accessibles via `Standardoc: …`,
plus des commandes exclusivement palette : **Find symbol**, **Get context
for symbol at cursor**, **Initialize workspace**, **Refresh .mcp.json
paths**, **Regenerate AI agent skill**, **Reset global init prompt**.

### Vérifier que l'agent voit bien Standardoc

Ton client MCP liste les servers connectés — Standardoc doit y apparaître
avec ses tools (`find_symbol`, `get_context`, `get_body`,
`find_call_sites`, `fetch_graph`, …). S'il n'y est pas : vérifie l'item du
status bar (daemon lancé ? sinon **Restart daemon**), relance `Standardoc:
Refresh .mcp.json paths` si le workspace a bougé, et lis l'output channel
`Standardoc` pour les erreurs de démarrage.

---

## 5. Sans l'extension VSCode (CLI standalone)

Standardoc marche avec n'importe quel client MCP-aware, sans
dépendance VSCode.

### Installer le binaire

**Pre-built binaries** (channel principal) — télécharge l'archive
matching ta plateforme depuis
[releases/latest](https://github.com/miralabs-tech/standardoc/releases/latest).
Le manifest `version.json` liste les archives par plateforme avec
SHA256 pour vérification.

**OU via cargo** (build source) :

```sh
cargo install --git https://github.com/miralabs-tech/standardoc standardoc-cli
standardoc --version
```

### Lancer les daemons

```sh
# Primary writer (acquiert le fs lock sur .standardoc/)
standardoc lsp /chemin/abs/workspace

# MCP en lecture seule, transport stdio (un client à la fois)
standardoc mcp /chemin/abs/workspace --readonly

# MCP en lecture seule, transport HTTP/SSE (multi-client)
standardoc mcp /chemin/abs/workspace --readonly --http 0
# Endpoint URL écrit dans .standardoc/mcp.endpoint
```

### Config MCP minimale (client stdio)

```json
{
  "mcpServers": {
    "standardoc": {
      "type": "stdio",
      "command": "/abs/path/to/standardoc",
      "args": ["mcp", "/abs/path/to/workspace", "--readonly"]
    }
  }
}
```

### Emplacements config courants

- **Claude Desktop** — `claude_desktop_config.json`
  (Settings → Developer → Edit Config)
- **Claude Code CLI** — `~/.claude.json` ou `.mcp.json` par projet
- **Cursor** — `~/.cursor/mcp.json` ou `.cursor/mcp.json`
  (workspace)
- **Autres clients MCP-aware** — voir leur doc respective

Plusieurs clients MCP `--readonly` peuvent attacher en concurrence
sur le même index SQLite sans contention. Le LSP tient le fs lock
du workspace comme writer principal.

---

## 6. Sub-commandes utiles

```sh
standardoc init <ws>                   # installe skill + hooks + AGENTS.md + .mcp.json
standardoc lsp <ws>                    # primary writer daemon
standardoc mcp <ws> --connect          # pont stdio↔http (ce que init écrit dans .mcp.json)
standardoc mcp <ws> --readonly         # readonly MCP daemon (stdio)
standardoc mcp <ws> --http <port>      # MCP daemon (HTTP/SSE)
standardoc index <ws>                  # one-shot index
standardoc watch <ws>                  # watcher seul
standardoc rescan <ws>                 # rebuild from scratch
standardoc query <ws> ...              # CLI query (find / context / body)
standardoc purge-excluded <ws>         # cleanup post-.stdignore edit
standardoc schema-version <ws>         # print schema version
standardoc sxd-preview <ws> <pattern>        # preview .stdignore matches
```

La surface MCP exposée par le daemon (`find_symbol`, `get_context`,
`get_body`, `get_code`, `find_call_sites`, `module_lookup`,
`fetch_graph`, `current_revision`, `check_stale`, plus la famille
cross-workspace `link_workspace` / `resolve_cross_workspace`) est
documentée en détail dans le `SKILL.md` auto-généré au workspace
init — c'est ce que l'agent lit pour savoir comment utiliser
Standardoc. Pas la peine de la mémoriser côté humain.

---

## 7. Régler l'index

### `.stdignore`

Auto-seedé à la racine du workspace à la première init.
**Syntaxe gitignore**, hot-reload des modifications.

Template par défaut : `.git/`, `node_modules/`, `target/`,
`dist/`, `build/`, `.old/`, `*-old/`, `test-export/`.

- **Ajouts** → excluent des chemins (purge automatique des
  symboles matchant via `standardoc purge-excluded`)
- **Retraits** → re-index automatique du sous-arbre concerné

---

## La suite

- **[Philosophie](storytelling/philosophy.md)** — les 5 principes
  system-thinking et l'éthique de construction
- **[Vision court terme](storytelling/vision-court-terme.md)** —
  beta.2 et la phase de stabilisation
- **[Vision moyen terme](storytelling/vision-moyen-terme.md)** —
  beta.3 et 1.0
- **[Remarques](storytelling/remarques.md)** — observations
  dogfood, posture, support
- **[FAQ](FAQ.md)** — questions courantes
- **[Comparaison](COMPARISON.md)** — vs LSP / Sourcegraph / autres
- **[Support](SUPPORT.md)** — comment soutenir le projet

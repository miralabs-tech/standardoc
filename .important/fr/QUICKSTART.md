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

> **À partir de beta.2** — ce flux arrive avec le découplage binaire ↔
> extension (item *Travail restant* de la [TODO-LIST](TODO-LIST.md)).
> Tant qu'il n'a pas shippé, l'extension embarque le binaire et tu peux
> sauter directement à [§3](#3-initialiser-un-workspace).

À la première activation, un modal te propose :

> **Download `standardoc` binary for `<platform>` ?** &nbsp; [OK] &nbsp; [Skip]

- **OK** → l'extension fetch `version.json` depuis
  `releases/latest/download/version.json`, télécharge l'archive
  matching ta plateforme, vérifie le SHA256, et l'installe dans
  le dossier d'extension (`bin/<platform>/standardoc[.exe]`).
- **Skip** → l'extension reste inerte (pas de daemon spawn). Une
  affordance dans la status bar te permet de relancer le download
  plus tard.

> *Pourquoi pas de binaire bundlé ?* Le VSIX serait gros
> (plusieurs dizaines de MB × N plateformes), et le binaire évolue
> à un rythme indépendant du cycle de release ext. Le decoupling
> permet de mettre à jour le binaire sans bumper l'extension —
> compat check via le champ `protocol_version` du manifeste
> `version.json`.

Une fois le binaire en place, l'extension supervise le daemon,
gère les redémarrages (parallel spawn, rollback
`Promise.allSettled`, backoff state machine), et enregistre
Standardoc comme MCP server pour Copilot Chat / Claude Code dans
VSCode.

---

## 3. Initialiser un workspace

Ouvre un projet (Rust / TypeScript / JavaScript / React (JSX & TSX) / Vue / Svelte / Lua).
Notification 4 boutons à la première activation :

> **Standardoc: Initialize this workspace?**
> [Initialize] [Skip] [Never for this workspace] [Never (any workspace)]

Click **Initialize**. L'extension :

1. Crée `.standardoc/` — index SQLite + RAG + métadonnées
2. Crée `.standardoc-sessions/` — memos agent cross-session
3. Écrit `.mcp.json` à la racine (cross-client merge, préserve
   les fields user existants)
4. Génère `.claude/skills/standardoc/SKILL.md` (~480 lignes —
   enseigne MCP-first, protocole 3-phase, edge kinds, 9
   workflows recommandés)
5. Spawn le LSP daemon (primary writer) + le MCP daemon HTTP/SSE
6. Cold start indexe ton workspace (5–15s suivant la taille,
   progrès visible via `$/progress` côté LSP)

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

L'init opt-in flow injecte **deux mécanismes** dans
`.claude/settings.json` du workspace :

**1. MCP-first guardrail** — empêche l'agent de dégénérer en
grep-loop. Trois hooks coordonnés :

- `SessionStart` *(reset)* — wipe le sentinel à chaque nouveau
  chat pour rester strict
- `PreToolUse` *(mark)* — pose le sentinel dès qu'un tool
  Standardoc est appelé
- `PreToolUse` *(check)* — bloque `Bash` / `Read` / `Grep` /
  `Glob` si le sentinel est absent

**2. Auto-sync sessions** *(`PostToolUse`)* — quand l'agent
écrit un memo dans son **dossier memory natif Claude**
(`~/.claude/projects/<hash>/memory/*.md`), le contenu est
auto-importé dans `.standardoc-sessions/sessions.db`.

> **Important : le hook se déclenche uniquement sur ces
> écritures-là.** Pas sur les `Write` / `Edit` / `MultiEdit` du
> code source ou de fichiers projet — seulement les fichiers
> dont le chemin matche `/.claude/projects/` ET `/memory/`
> (les constantes `MEMORY_PATH_MARKER` + `MEMORY_PATH_TAIL`
> côté `standardoc-cli`).

**Bridge automatique entre la memory native Claude Code et la
sessions DB Standardoc** — l'agent capitalise ses memos sans
avoir à les ré-écrire via `session_save` à chaque fois.

Si tu utilises un autre client MCP-aware, configure tes propres
hooks équivalents :

```sh
standardoc claude pre-tool-hook --mode mark   # poser le sentinel
standardoc claude pre-tool-hook --mode check  # bloquer Bash/Grep/...
standardoc claude pre-tool-hook --mode reset  # wipe au SessionStart
standardoc session hook                        # auto-import memo (PostToolUse)
```

### Sessions persistantes (cross-chat memory)

`.standardoc-sessions/sessions.db` est **créée automatiquement
au premier appel d'un tool `session_*`** par l'agent — aucune
action humaine requise sur l'init de la DB. La DB est
volontairement séparée de `.standardoc/` pour qu'un reset du
graphe (ou un `Standardoc: Rebuild RAG index`) ne wipe pas les
memos.

**L'initiation de la convention reste à l'opérateur.** L'agent
ne crée pas spontanément des memos — il sait le faire parce
que le skill auto-généré documente le workflow, mais il attend
qu'on lui dise. Il faut **briefer l'agent au premier chat** :

> *« Organise-toi en sessions. Save un memo `session_save(slug,
> body_md)` à la fin de chaque chantier ou décision lockée. Au
> début du chat suivant, fais `session_get()` pour récupérer
> où on en était. »*

Une fois la convention établie dans les premiers chats, l'agent
la maintient tout seul via :

- **Fin de session** (locke des décisions ou ship du travail) :
  `session_save(slug, body_md, supersedes?)` pour persister un
  memo. Le `supersedes` chaîne les memos quand un refactor
  invalide un lock précédent.
- **Début de la session suivante** : `session_get()` (sans
  slug) retourne le memo le plus récent actif comme point
  d'entrée.
- `session_list({active_only: true})` pour scanner les memos
  récents.

Quatre kinds distincts via le champ `type` du frontmatter :
`session` (handoff par défaut), `feedback` (règles
comportementales), `profile` (facts user stables), `lock`
(décisions lockées — équivalent **ADR** au format memo).

**Migration manuelle d'un dossier `.md` externe** :

```sh
# Importer un dossier .md → sessions.db
standardoc session sync-in /chemin/workspace /chemin/dossier-memos

# Exporter sessions.db → dossier .md (frontmatter complet)
standardoc session sync-out /chemin/workspace /chemin/dossier-export
```

### Le menu du status bar (clic en bas à droite)

L'item Standardoc dans le status bar ouvre un QuickPick avec
**les actions courantes** :

- **▶ Start daemon** / **■ Stop daemon** / **↻ Restart daemon**
- **🗑 Purge excluded paths** — purge les symboles dont le
  fichier source matche désormais `.stdignore`
- **Enable / Disable RAG** *(toggle dynamique selon l'état)*
- **Switch RAG embedder…** — choix entre :
  - **Mock** : déterministe, zéro réseau (pour le dev / tests)
  - **Candle (BGE-small)** : BERT 384-dim local. Premier run :
    téléchargement ~130 MB (cache `~/.cache/standardoc/models/`,
    override via la variable d'env `STANDARDOC_MODELS_DIR`)
- **Rebuild RAG index** — stoppe le daemon, supprime
  `.standardoc/rag.db` (+ `-wal` / `-shm`), redémarre. Les
  chunks sont ré-embeddés au cold start. Modal de confirmation
  avant exécution.
- **Show token savings** — affiche le ratio `bytes_out /
  baseline_bytes` (ce que Standardoc a renvoyé vs. ce que
  l'agent aurait lu en raw) par période (today / day / week /
  all)
- **Reset token savings…** — baseline une mesure propre

### Palette de commandes VSCode (`Ctrl+Shift+P`)

Toutes les actions du status bar menu sont accessibles à la
palette via `Standardoc: …`, plus quelques commandes
exclusivement palette :

- `Standardoc: Find symbol` — InputBox + QuickPick sur
  `find_symbol`, ouvre le symbole choisi à sa source
- `Standardoc: Get context for symbol at cursor` —
  `get_context(depth=1)` rendu dans l'output channel
- `Standardoc: Initialize workspace` — re-déclenche l'init
  opt-in flow (utile si `.standardoc/` a été supprimé)
- `Standardoc: Refresh .mcp.json paths` — re-merge avec les
  chemins absolus courants après déplacement du workspace ou
  rebuild du binaire ailleurs
- `Standardoc: Regenerate AI agent skill` — overwrite
  `.claude/skills/standardoc/SKILL.md` (utile après upgrade ext)
- `Standardoc: Reset global init prompt` — ré-arme la
  notification 4-boutons même sur les workspaces où *Never* a
  déjà été cliqué

### Vérifier que l'agent voit bien Standardoc

Côté client MCP (Copilot Chat / Claude Code dans VSCode, Cursor,
etc.), chaque client a sa propre UI pour lister les MCP servers
connectés et leurs tools disponibles. Standardoc doit y
apparaître avec ses **16 tools** (`find_symbol`, `get_context`,
`get_body`, `fetch_chunks`, `session_save`, etc.).

Si l'agent dit que Standardoc n'est pas disponible :

1. **Status bar** — l'item indique-t-il que le daemon tourne ?
   Sinon, **Restart daemon** depuis le menu.
2. **`.mcp.json`** — les chemins absolus sont-ils encore valides
   (workspace déplacé, binaire mis à jour) ? Lance `Standardoc:
   Refresh .mcp.json paths`.
3. **L'output channel `Standardoc`** affiche les logs du daemon
   et de la supervision — c'est là qu'on voit les erreurs de
   démarrage, les markers `STDOC_FATAL`, les DL d'embedder, etc.

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
standardoc lsp <ws>                    # primary writer daemon
standardoc mcp <ws> --readonly         # readonly MCP daemon (stdio)
standardoc mcp <ws> --http <port>      # readonly MCP daemon (HTTP/SSE)
standardoc index <ws>                  # one-shot index
standardoc watch <ws>                  # watcher seul
standardoc rescan <ws>                 # rebuild from scratch
standardoc query <ws> ...              # CLI query (find / context / body)
standardoc purge-excluded <ws>         # cleanup post-.stdignore edit
standardoc reset-usage --period <p>    # reset usage_stats (today/day/week/all)
standardoc schema-version <ws>         # print schema version
standardoc session sync-in <ws> <dir>  # bridge .md memos → sessions.db
standardoc session sync-out <ws> <dir> # bridge sessions.db → .md memos
standardoc stdignore-preview <ws> <pattern>  # preview .stdignore matches
```

La surface MCP exposée par le daemon (**16 tools** :
`find_symbol`, `get_context`, `get_body`, `fetch_chunks`,
`session_save`, `current_revision`, `check_stale`, `usage_stats`,
etc.) est documentée en détail dans le `SKILL.md` auto-généré au
workspace init — c'est ce que l'agent lit pour savoir comment
utiliser Standardoc. Pas la peine de la mémoriser côté humain.

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

### RAG sur la prose adjacente

`docs/`, `notes/`, et les `*.md` au root + aux sous-package roots
sont chunkés et accessibles via les `chunk_refs` de `get_context`,
ou directement via `fetch_chunks(uri)`.

L'embedder par défaut est **Candle BGE-small** (~130 MB, lazy
download au premier usage RAG). **Tourne en local, pas de cloud.**
Les chunks vivent dans `.standardoc/rag.db`, linkés au graphe par
FQDN.

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

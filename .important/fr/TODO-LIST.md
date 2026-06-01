# Roadmap

[English](../en/TODO-LIST.md) · 📖 Français

Source de vérité de ce qui est planifié, de ce qui est en cours de
livraison, et de ce qui est délibérément reporté.
[`CHANGELOG.md`](../../CHANGELOG.md) trace ce qui a effectivement
shippé par release ; ce fichier-ci trace l'intention.

Convention des cases : `[x]` shippé · `[ ]` planifié · `~~barré~~`
abandonné ou reporté vers un milestone ultérieur.

---

## Politique de versioning

Standardoc a deux livrables de release qui évoluent à des cadences
différentes :

- **Core** — distribué en binaires pre-built (binaire unique
  `standardoc`). Tag-driven : pousser `vX.Y.Z` déclenche `release.yml`
  (binaires pre-built cross-platform + manifeste `version.json` +
  GitHub Release). Builds source via
  `cargo install --git https://github.com/miralabs-tech/standardoc`.
- **Extension** — publiée sur le VSCode Marketplace + Open VSX.
  Déclenchement manuel via `release-ext.yml` (workflow_dispatch avec
  inputs `version` + `pre_release`). Découplée du tag push.

### Cadence

| Phase                    | Politique                                                                                       |
| ------------------------ | ----------------------------------------------------------------------------------------------- |
| `1.0.0-betaN` / `-rcN`   | **Lockstep**. Un seul tag pilote les deux à la même version. Le core bouge vite, l'ext est étroitement couplée. |
| `1.0.0` et au-delà       | **Indépendance par défaut.** Chaque composant bump à son propre rythme.                         |

### Règles d'indépendance (post-`1.0.0`)

- **Bump MAJOR** sur le core OU l'ext → resync obligatoire. Les deux
  bump ensemble. Reflète un breaking change dans le protocole IPC ou un
  changement de contrat qui invalide les contreparties plus anciennes.
- **Bump MINOR** sur l'un ou l'autre → indépendant. De nouveaux tools
  ou de nouvelles capacités côté core peuvent atterrir sans forcer
  l'ext à se mettre à jour ; l'ext peut itérer son UI/UX sans bumper le
  core.
- **Bump PATCH** sur l'un ou l'autre → totalement indépendant.

### Manifeste `version.json`

Chaque release du core attache un `version.json` aux artefacts de la
GitHub Release :

```json
{
  "core_version": "1.0.0-beta.1",
  "ext_version": "1.0.0-beta.1",
  "protocol_version": 1,
  "min_compat": { "core": "1.0.0-beta.1", "ext": "1.0.0-beta.1" },
  "released_at": "2026-05-XX",
  "binaries": { "x86_64-unknown-linux-gnu": "https://.../standardoc-...tar.gz", ... },
  "checksums_sha256": { "x86_64-unknown-linux-gnu": "abc123...", ... }
}
```

URL stable : `https://github.com/miralabs-tech/standardoc/releases/latest/download/version.json`.

Le futur sélecteur de version de l'ext consomme ce fichier pour
peupler le réglage `standardoc.coreVersion` et télécharger à la demande
les binaires `standardoc` correspondants.

### Protocol version

`protocol_version` (actuellement `1`) est découplé du semver. Il trace
le contrat IPC (signatures des tools MCP, méthodes LSP custom) et ne
bump que sur de vraies cassures de wire-format. L'ext vérifie
`standardoc --version` (qui expose la protocol version) au boot ; un
mismatch déclenche un toast d'avertissement.

---

## v1.0.0-beta.1 — Foundation

**Thème** : graphe sémantique AST-direct + surface MCP/LSP + extension
VSCode. Rust + TypeScript uniquement. Deux tools MCP. Local-only.

### Livré

#### Core data layer
- [x] Providers AST-direct (Rust via `syn`, TypeScript via `swc`)
- [x] IR canonique (symboles FQDN-keyed, edges typés, variant `ResolvedOrUnresolved`)
- [x] Stockage graphe SQLite + FTS5 (external content zéro-duplication)
- [x] Invalidation BLAKE3 à double niveau
- [x] Unification FQDN cross Rust + TS (`<package>::<module>::<name>`)
- [x] Edges typés day-1 : `CALLS`, `IMPORTS`, `EXTENDS`, `IMPLEMENTS`, `REFERENCES`, `USES_TYPE`

#### Pipeline
- [x] Index full-workspace eager au cold-start
- [x] File watcher live avec auto-rescan débouncé (`notify` + `notify-debouncer-full`)
- [x] `.stdignore` (syntaxe gitignore) + auto-seed à la racine du workspace
- [x] Hot-reload du `.stdignore` (diff/swap/warn/reindex)
- [x] Sub-commande `purge-excluded` pour le cleanup post-édition de `.stdignore`
- [x] Handle pause / resume de l'index

#### CLI / Daemons
- [x] Binaire unique `standardoc` avec sub-commandes (`lsp`, `mcp`, `index`, `rescan`, `watch`, `query`, `purge-excluded`)
- [x] Daemon LSP = writer principal (acquiert le fs lock via `fs4`)
- [x] Daemon MCP `--readonly` (`SQLITE_OPEN_READ_ONLY`, skip du fs lock)
- [x] Plusieurs clients MCP `--readonly` peuvent attacher en concurrence
- [x] LSP : hover, document/workspace symbols, navigation, $/progress au cold start
- [x] 2 tools MCP : `find_symbol(query, limit?)`, `get_context(fqdn, depth=1|2)`

#### Extension VSCode
- [x] Superviseur de daemons (LSP + MCP, parallel spawn, rollback `Promise.allSettled`)
- [x] Backoff state machine pour les redémarrages de daemon
- [x] Item dans la status bar
- [x] Init opt-in flow (notification 4 boutons, memento per-workspace + global)
- [x] Merge cross-client de `.mcp.json` (5 actions discriminées, préserve les champs user)
- [x] Génération du skill agent IA (`.claude/skills/standardoc/SKILL.md`)
- [x] MCP server provider pour Copilot Chat / Claude Code dans VSCode
- [x] Palette de commandes : Find symbol, Get context, Daemon Stop/Start/Restart, Init, Refresh `.mcp.json`, Regenerate skill, Reset global init prompt, Purge excluded

#### Distribution & infra
- [x] Config Renovate (conservative, hebdomadaire, group minor/patch, automerge GHA uniquement)
- [x] CI : fmt + clippy + test cross-OS + docs + ext (bun test/tsc/build)
- [x] Workflow de sync des labels

### Publié ✓

- [x] Audit des flags `publish` du workspace `Cargo.toml`
- [x] Deps path-only vérifiées comme ayant un champ `version` (alignées sur `1.0.0-beta.1`)
- [x] `bridge-sdk` aligné sur `version.workspace = true`
- [x] `cargo publish --dry-run` validé sur les leaf crates
- [x] ~~Première chaîne de publish sur crates.io~~ — abandonné ; distribution = binaires pre-built GitHub Release + `cargo install --git` (publish crates.io reporté, aucun engagement ferme)
- [x] Push du tag `v1.0.0-beta.1` → `release.yml` (binaires cross-platform + `version.json` + GitHub Release)
- [x] Déclenchement de `release-ext.yml` workflow_dispatch (`version=1.0.0`, `pre_release=true`)
- [ ] Smoke F5 full E2E re-test post init opt-in flow + skill gen
- [ ] Annonce publique de la roadmap (linker ce fichier depuis le README + GitHub Discussions)

---

## Cycle v0.x.x — entre beta.1 et v1.0.0

**Thème** : dogfood, fix, harden. Ouvert aux premiers language providers
additionnels une fois que le feedback user valide la fondation.

- [ ] Premier language provider post-Rust+TS (Python via `rustpython-parser` ou tree-sitter)
- [ ] Fix du schéma de collision fqdn cross-folder (`module_path` composite UNIQUE)
- [ ] Optimisation de skip incrémental basée sur le mtime
- [ ] Roadmap publique sur GitHub Discussions / project board
- [ ] Politique `SECURITY.md`
- [ ] Formalisation du Code of Conduct (Contributor Covenant)

---

## v1.0.0-beta.2 — Hardening + raffinement de la surface MCP

**Thème** : éprouver la fondation sous des charges agent réelles. La
surface MCP 2-tools day-1 de beta.1 grossit en un toolkit agent plus
large ; le transport HTTP/SSE atterrit ; la couverture langages triple
(Lua, Vue, Svelte ajoutés ; Rust + TS hardenés) ; la résilience du
daemon encaisse l'orchestration de processus réelle. Aucun nouveau crate
public ni package npm — ceux-là atterrissent en beta.3.

> **Re-scopé en beta.3.** beta.2 a aussi shippé une couche de retrieval
> RAG, une DB de session handoff, et une surface usage-stats /
> token-savings. beta.3 a retiré ou extrait les trois — RAG (la
> résolution structurelle bat la similarité vectorielle), sessions
> (déplacée vers un outil voisin de session-store), usage-stats
> (abandonné). Elles sont barrées plus bas et listées sous
> [Reporté / abandonné](#reporté--abandonné).

### Livré

#### Core data layer
- [x] Schema v6 : revision du workspace persistée, handle R/W secondaire, colonne edge confidence
- [x] ~~Table `usage_stats` (schema v2) + API de query `log_usage`~~ — retiré en beta.3
- [x] Rendu d'affichage compact pour les chaînes de type / attributs (dérivé Rust `to_token_stream` plus generic neutralizer)

#### Surface des tools MCP — expansion depuis les 2 tools day-1
- [x] **Symbol discovery** — `find_symbol` (FTS5 + fallback did_you_mean), `find_symbols_by_pattern` (GLOB), `find_similar_symbols` (strsim), `list_symbols` (filter-only)
- [x] **Context** — `get_context` avec sémantique `depth=1|2` ; le `routing_hint` corrige spécifiquement le **pacing depth=1 → depth=2** (se déclenche quand depth=2 est appelé sur un fqdn sans depth=1 récent dans les 5 min, silencieux sinon)
- [x] **Body** — `get_body` avec knobs `max_lines`, `strip_attrs`, `signature_only` et output compact common-prefix-dedent + tab-indent
- [x] **Externals** — `resolve_external` pour les lookups externes cross-language
- [x] **Capabilities & freshness** — `current_revision` expose `{watcher.active, indexing.ready}` ; `check_stale` pour l'invalidation des fqdn cachés
- [x] ~~**RAG** — `fetch_chunks`~~ · ~~**Telemetry** — tool `usage_stats`~~ · ~~**Sessions** — `session_save` / `session_list` / `session_get` / `session_sync_in` / `session_sync_out`~~ — retirés ou extraits en beta.3
- [x] Sanitization des queries FTS5 (gère snake_case, camelCase, tokens partiels, fallback strsim did_you_mean au seuil 0.6)
- [x] Normalisation FQDN OOP-style à la frontière MCP (`Class.method` → `Class::method`)

#### Transport MCP HTTP/SSE
- [x] Transport streamable-http (multi-client, découplé du child-spawn stdio)
- [x] Flag CLI `standardoc mcp --http <port>` ; `--http 0` laisse le kernel choisir un port éphémère
- [x] URL de l'endpoint écrite dans `.standardoc/mcp.endpoint` pour la découverte client
- [x] Auto-fallback de port sur `EADDRINUSE` (pas de marker fatal, log d'avertissement seulement)
- [x] Parent death-watch via EOF stdin (TTY-guarded) — élimine les locks de workspace orphelins lors d'un crash du superviseur
- [x] Boot binary sweep (détecte les processus `standardoc.exe` orphelins de runs précédents)
- [x] Boot lockfile invalidation sweep (récupère après des locks fs4 stale)
- [x] Protocole de marker `STDOC_FATAL: <code> <key>=<value>` pour la reconnaissance superviseur-side d'une config fatale

#### Couche RAG (retrieval de prose) — ~~retirée en beta.3~~
- [x] ~~crate `standardoc-rag` (chunker, embedder, store, linker, score) ; découverte prose-convention ; BGE-small-en-v1.5 via Candle ; invalidation BLAKE3 des chunks ; `relink_watcher` re-anchrant sur changement du graphe ; cold-start piloté par le LSP~~ — shippée en beta.2, **retirée en beta.3** : Standardoc parie sur la résolution structurelle, pas la similarité vectorielle.

#### DB de session handoff — ~~extraite en beta.3~~
- [x] ~~`.standardoc-sessions/sessions.db` ; discriminateur `SessionKind` (`session` / `feedback` / `profile` / `lock`) ; sync `.md` ↔ DB bidirectionnel ; CLI `standardoc session {sync-in,sync-out,hook}`~~ — shippée en beta.2, **extraite en beta.3** vers un outil voisin de session-store ; le core ne porte plus la mémoire agent.

#### Guardrail MCP-first
- [x] Driver CLI `standardoc claude pre-tool-hook --mode {mark,check,reset}`
- [x] Hook PreToolUse refuse `Bash|Read|Grep|Glob` tant qu'un tool MCP standardoc n'a pas été appelé
- [x] Hook SessionStart wipe le sentinel pour que chaque nouveau chat démarre strict
- [x] Cross-OS via le binaire dans le PATH (pas d'adaptation shell-script, pas de détection d'OS dans la couche TS)

#### Externals (cargo / npm / luarocks)
- [x] Resolvers externes lazy on-demand — pas de pre-walk des deps vendorées au moment de l'indexation
- [x] Découverte de manifeste walk-down (`Cargo.toml`, `package.json`, `*.rockspec`)
- [x] Tool MCP `resolve_external` surface les métadonnées résolues aux agents
- [x] Surface de test d'intégration E2E

#### Usage stats / token savings — ~~retiré en beta.3~~
- [x] ~~logging per-tool du read-handler dans `usage_stats` ; tool de query MCP `usage_stats` ; CLI `standardoc reset-usage` ; commande token-savings VSCode + status bar ; angle savings du skill template~~ — shippé en beta.2, **retiré en beta.3**.

#### Language providers
- [x] **Provider Lua natif** (`full_moon`) : fonctions, locals, module tables (`M = {}`), imports `require`, edges d'appel, extraction d'annotations emmylua
- [x] **Vue SFC** (`.vue`) : extraction `<script>` / `<script setup lang="ts">` → `TsProvider` ; edges de ref `<template>` avec attributs (nom du composant, bindings de props, kind de slot)
- [x] **Composants Svelte** (`.svelte`) : pipeline script-extract, attributs de ref de template
- [x] **Hardening Rust** : phantoms `pub use` (chaîne de visibilité des re-exports), skip d'impl sur types non-nominaux, `module_path` rendu crate-relative
- [x] **Unification TS visit + SFC** : schéma FQDN consistant cross `.ts` / `.tsx` / `.vue` / `.svelte`
- [x] Module `utils` partagé entre providers (helpers FQDN, primitives de walk communes)
- [x] Champ `attributes` des edges — métadonnées structurées pour les refs de template

#### Hardening pipeline & storage
- [x] `IndexHandle::open` retry avec backoff exponentiel sur erreurs de lock transitoires (`SQLITE_PROTOCOL`, `database is locked`, `database is busy`, timeout r2d2 nu)
- [x] Pool de connexions r2d2 : lazy init (`min_idle = 0`), timeout 10 s, helper de retry qui cycle vite
- [x] Pass de cleanup pour les fichiers non vus (maintient la contrainte XOR CHECK après édition de `.stdignore`)

#### Extension VSCode
- [x] Daemons LSP + MCP HTTP supervisés (parallel spawn, rollback `Promise.allSettled`, backoff state machine)
- [x] Init opt-in flow (notification 4 boutons, memento per-workspace + global, re-prompt à la suppression de `.standardoc/`)
- [x] Génération du skill agent IA (`.claude/skills/standardoc/SKILL.md`) avec table de couverture langages et documentation des edge-attributes
- [x] MCP server provider pour Copilot Chat / Claude Code ; merge cross-client de `.mcp.json` (5 actions discriminées, préserve les champs user)
- [x] `.mcp.json` réécrit vers l'URL réelle du daemon à chaque transition `ready` (couvre le fallback de port éphémère)
- [x] Contribution langage `.stdignore` + hover preview gitignore-style
- [x] ~~Commandes RAG dans la palette + réglages + status bar + fix de race sur l'endpoint + markers de progrès DL~~ — retiré en beta.3
- [x] Redémarrage de daemon sérialisé
- [x] Gestion des erreurs fatales parse les markers `STDOC_FATAL` (pas de regex sur les messages d'erreur en prose)
- [x] ~~Commande token savings + item dans la status bar~~ — retiré en beta.3

#### Infra
- [x] Hardening CI : cleanup `cargo fmt --all` du workspace, fix des intra-doc-links cassés, fix `clippy::format_push_string` / `match_same_arms`
- [x] CI basculée de `actions-rust-lang/setup-rust-toolchain` vers `dtolnay/rust-toolchain` (fiabilité macos-latest)
- [x] Resserrage des permissions du workflow code-scanning (5 PR auto-fix mergées)
- [x] `release.yml` simplifié (steps de publish crates.io retirés)
- [x] Fix du champ Cargo `package.publisher`
- [x] `.gitignore` couvre `.standardoc-sessions/`, `sessions-export/`, `.claude/`, `.mcp.json`, `ext/vscode/.standardoc/`
- [x] README + SECURITY.md + SUPPORT.md rafraîchis (détails AST + install, versions supportées, audit des liens)

### Travail restant

- [x] **Découpler le binaire `standardoc` du VSIX de l'extension VSCode**
  - Le VSIX shippe sans `standardoc[.exe]`. À la première activation
    sans binaire résolvable, le superviseur transite vers un nouvel
    état `awaiting_binary` (distinct de `failed` pour que le
    retry/backoff reste désarmé).
  - Toast surface `Standardoc needs to download the native binary for
    this platform.` avec `Download` / `Later` / `Show logs`. La status
    bar affiche une affordance `$(cloud-download) Standardoc` qui
    bypass le menu habituel et route un clic directement vers la
    commande de download.
  - Sur `Download` : l'installer fetch
    `releases/download/v<BINARY_VERSION>/version.json` (épinglé via le
    `binary-version.ts` de l'ext, pas `latest`), pick l'asset pour le
    `process.platform + process.arch` courant mappé vers le target
    triple Rust, télécharge l'archive, vérifie le SHA256, extrait via
    le `tar` système, et écrit le binaire dans
    `<globalStorageUri>/bin/<rust-target-triple>/standardoc[.exe]`.
  - L'ordering de `binary-resolver.ts` est `settings → globalStorage
    → PATH → throw`. `standardoc.binaryPath` reste l'échappatoire
    documentée pour le dev local (`target/debug/standardoc`) et le
    pinning pre-release.
  - Le script `bundle-binary.ts` et la chaîne `dev:bundle` /
    `package` qui copiaient `target/release/standardoc[.exe]` dans le
    VSIX sont supprimés.
  - La vérification compat `protocol_version` ride sur le SHA256 : un
    SHA matchant prouve que le binaire est bien celui publié sous le
    tag pinné, ce qui prouve le contract du protocole. Exposer
    `protocol_version` via `standardoc --version --json` comme
    cross-check runtime reste un item post-1.0.
  - Effet net : la taille du VSIX baisse de dizaines de MB ; les
    updates du binaire roulent indépendamment de la cadence de release
    de l'ext ; s'aligne avec la plomberie `self-update` de beta.3 (même
    chemin de consommation du `version.json`).

- [ ] **Audit de la racine du repo + réorg dans `.important/`**
  - Déplacer les docs long-form dans un répertoire top-level
    `.important/` (volontairement eye-catching dans le listing de
    fichiers GitHub pour que les nouveaux le remarquent depuis le hub
    du README).
  - Fichiers déplacés : `ABOUT(.fr).md`, `QUICKSTART(.fr).md`,
    `FAQ(.fr).md`, `COMPARISON(.fr).md`, `SUPPORT(.fr).md`,
    `TODO-LIST.md`.
  - Garder à la racine uniquement ce que GitHub surface par
    convention : `README.md`, `LICENSE`, `SECURITY.md`, `CHANGELOG.md`
    (+ `CONTRIBUTING.md` si/quand ajouté).
  - Le `README.md` reçoit une section « Navigate » linkant chaque doc
    déplacé (paire en + fr par ligne), pour que le chemin de
    click-through soit en un saut.
  - Mettre à jour les références de liens entrants partout : cross-refs
    du README, liens de `SUPPORT.md`, URL de documentation du
    `package.json` de l'ext, liens des release notes, mentions de
    fichiers in-repo.
  - Rafraîchir le contenu pendant le déplacement — tout ce qui est
    stale ou en framing pre-beta.1 passe une révision.

- [ ] **Fixer `renovate.json`** — actuellement non fonctionnel,
  diagnostiquer depuis zéro
  - Confirmer que la GitHub App Renovate est installée sur
    `miralabs-tech/standardoc` (Settings → Apps).
  - Valider la config `renovate.json` existante via
    `npx --package renovate -c renovate-config-validator`.
  - Reconfigurer pour cibler `dev` (pas `main` — `main` est
    branch-protected et les PR Renovate contre elle seraient rejetées) :
    poser `"baseBranches": ["dev"]`.
  - Déclencher un dry-run hosted-app via l'issue dependency-dashboard ;
    inspecter le log pour la vraie raison des runs passés sans résultat.
  - Vérifier en attendant que le prochain run planifié produise une PR
    contre `dev`.

### Ops de release (en attente)

- [ ] Entrée `CHANGELOG.md` pour v1.0.0-beta.2 résumant ce qui précède
- [ ] Bump du `version` workspace de `Cargo.toml` → `1.0.0-beta.2` ; sync des refs de version des membres dans `[workspace.dependencies]`
- [ ] Bump du `version` de `ext/vscode/package.json` selon la politique de cadence
- [ ] Push du tag `v1.0.0-beta.2` → `release.yml` se déclenche (binaires cross-platform + `version.json` + GitHub Release)
- [ ] Workflow_dispatch `release-ext.yml` (`version=<ext-version>`, `pre_release=true`)
- [ ] Smoke F5 full E2E re-test post init opt-in flow + hooks MCP-first + skill gen
- [ ] Annonce publique de la roadmap (linker ce fichier depuis le README + GitHub Discussions)

---

## v1.0.0-beta.3 — Graphes multi-workspace · navigation visuelle · provider C

**Thème** : pluraliser les consommateurs du graphe. Le *plan* était doc
rendering + navigation visuelle + autonomie CLI ; en pratique le dogfood a
tiré beta.3 vers **les graphes multi-workspace / multi-root, la
visualisation interactive, un provider C natif, un proxy multi-workspace, et
une refonte profonde de la résolution d'edges** — pendant que RAG et la
session DB étaient coupés. **Navigation visuelle et autonomie CLI ont
shippé ; le doc rendering a glissé** vers une beta ultérieure. La surface
complète shippée vit dans [`CHANGELOG.md`](../../CHANGELOG.md) ; les axes
planifiés ci-dessous sont annotés avec ce qui a atterri.

### Couche de documentation rendering — ~~glissée après beta.3~~

Le doc graph (SQLite) est la source de vérité universelle. Les
renderers sont des consommateurs — MDX est une option, pas la base. Le
même graphe doit être consommable par n'importe quel framework sans
dépendance MDX.

Pipeline cible :

```
code source → parser @doc → doc graph (SQLite) → query API framework-agnostic → renderer
```

**Doc graph & query layer** (framework-agnostic, ship en `@standardoc/core`) :
- [ ] Ajouts au schéma du doc-graph (`description`, `examples`, `tags`) sans réintroduire de DSL custom
- [ ] Parser d'annotations (`@doc`, `@param`, `@returns`, `@example`) avec hooks language-provider
- [ ] `queryDocs("api.*")` — helper de query glob exposé en API plain JS/TS (aucun framework requis)

**Renderer React** (ship en `@standardoc/react`, premier renderer) :
- [ ] `<Doc id="…" />` — rendu d'un bloc de doc unique
- [ ] `<Params id="…" />` — table de paramètres
- [ ] `<Examples id="…" />` — snippets d'exemple
- [ ] `<Signature id="…" />` — signature en code-fence
- [ ] Adapters drop-in pour Next.js, Nextra, Astro, Docusaurus

**Renderers futurs** (post-beta.3, même graphe, packages différents) :
- [ ] `@standardoc/vue` — mêmes composants pour Vue / VitePress / Nuxt
- [ ] `@standardoc/svelte` — pour SvelteKit, Svelte plain

### Navigation visuelle — ✅ shippée (`standardoc-graph-viz`)

Surface le graphe comme un artefact visuel interactif pour le mainteneur
qui review/audite son propre code. Shippée comme le crate WASM
`standardoc-graph-viz` + shell en web-components, hébergée dans une webview
VSCode et un playground standalone, pilotée via MCP.

- [x] Shell graphe : overview (topologie 3D + clusters projet) / focus-graph / explorer / symbol-details / search
- [x] Click-to-navigate (drill dans les voisins, breadcrumb retour)
- [x] Vue compacte des enrichissements (signatures / champs / relations sans ouvrir de fichiers)
- [x] Filter chips `kind` / `visibility` / langue + hide-tests

### Self-management du CLI (`standardoc` sans VSCode) — ✅ en partie shippé

- [x] `standardoc init` (skill agent + hooks MCP-first + `AGENTS.md` + `.mcp.json`) + `standardoc mcp --connect` (pont stdio↔http) — wiring agent hors-VSCode first-class
- [x] Sub-commande `standardoc self-update` : lit `version.json` depuis les GitHub Releases, détecte la plateforme, télécharge + SHA256-vérifie le binaire correspondant, remplace l'exécutable courant (rename-on-replace Windows-aware)
- [ ] Injection PATH à l'install initiale : place le binaire sous `~/.stdoc/bin/` (Unix) ou `%USERPROFILE%\.stdoc\bin\` (Windows) et enregistre le path dans :
  - bash/zsh : append `export PATH="$HOME/.stdoc/bin:$PATH"` dans `.bashrc` / `.zshrc`
  - PowerShell : append dans `$PROFILE`
  - CMD / Windows permanent : écrit dans `HKCU\Environment\Path` via le crate `winreg`
- [ ] Scripts de bootstrap one-liner : `curl -sSf https://… | sh` (Unix) + `irm https://… | iex` (PowerShell)

### Compréhension projet cross-session — ~~extraite vers un outil voisin~~

La compréhension synthétisée du projet qu'un agent recharge à chaque
session (objectifs, posture, décisions lockées, intention narrative) ne
vit plus dans le core de Standardoc. Elle a été déplacée vers un **outil
voisin de session-store**, en même temps que l'extraction de la DB de
session handoff en beta.3 — le core reste un graphe de code, pas un
store de mémoire agent. Le garde-fou est inchangé : une telle synthèse
est une projection dérivée re-validée contre le graphe, jamais une
source de vérité indépendante.

---

## v1.0.0 — Stabilisation

**Thème** : API freeze. Maturité perf et opérationnelle avant de
verrouiller la surface.

- [ ] Enrichissements virtual annotations (conventions verb-prefix, narratives type-signature, templates trait impl)
- [ ] **Bridge kinds cross-substrat** — fermer le vocabulaire `BridgeKind` (`"tauri"`, `"wasm"`, `"ffi"`, `"sql"`, `"orm"`, `"db-table"`, `"db-model"`, et tout ce que le dogfood surface) avant le freeze 1.0. L'extension de vocab effective peut shipper sur n'importe laquelle des `beta.X` entre maintenant et 1.0 (dogfood-driven, pas calendar-driven). **Les détecteurs frontends ne sont PAS shippés à 1.0** — ils atterrissent post-1.0 via le plug-in layer UST + Lua (la combinatoire substrat × langage × ORM est trop large pour le core)
- [ ] Transport MCP HTTP/SSE pour daemon partagé multi-machine
- [ ] Benchmarks de performance sur monorepos 1M+ LOC
- [ ] Freeze de la surface API documenté + premier contrat stable

---

## Idées post-1.0 (aucun engagement)

- [ ] Language providers additionnels (Go, Java, Swift, C#, Kotlin, Zig) — Lua, Vue, Svelte shippés en beta.2
- [ ] Méthodes LSP custom pour les queries spécifiques à Standardoc
- [ ] UI de doc locale optionnelle façon GitBook (si la demande émerge ; licence à vie)
- [ ] Bridge LSP vers rust-analyzer / tsserver pour une profondeur per-langage plus riche
- [ ] **Import/export de commentaires de code via pointeurs safe-edit FQDN-ancrés** — remplacer la commande `materialize` abandonnée par une primitive plus rigoureuse : réécrire les doc-comments / annotations / blocs `@doc` dans le code source, ancrés sur des locations FQDN (plus stables que les line ranges brutes à travers les refactors). But : maintenir des codebases épurées (signatures + body, pas de murs de commentaires) tout en gardant la prose dans le graphe, avec ré-injection safe à la demande et aucun risque de désync entre graphe et source.

### Language provider universel : couche de scripting UST + Lua

**Vision** : Standardoc arrête de comprendre les langages directement —
il comprend une représentation normalisée universelle (UST), et Lua
définit comment chaque langage se mappe dedans. Ajouter un nouveau
langage devient écrire un plugin Lua, pas un backend Rust.

```
code source
  → parser (tree-sitter / n'importe quel tool) → UST (AST normalisé language-agnostic)
  → plugin Lua (définit symboles, relations, edges)
  → Rust valide + stocke dans le graphe IR
```

- [ ] **Spec UST** : définir un schéma de nœud minimal language-agnostic (kind, name, span, children, attributes) que tous les parsers produisent
- [ ] **Runtime Lua** (`mlua` embarqué) : sandbox qui reçoit un arbre UST et retourne `Vec<IrSymbol>` + `Vec<IrEdge>`
- [ ] **Intégration tree-sitter** : front-end de parser universel ; les grammars communautaires couvrent 100+ langages sans nouvelles deps Rust
- [ ] **Découverte de plugins** : `.standardoc/plugins/<lang>.lua` auto-chargé par workspace
- [ ] Remplace / complète l'approche WASM bridge pour les language providers communautaires (Lua = barrière plus basse que WASM ; WASM gardé pour les plugins natifs full performance)

### Sélecteur de version de l'extension VSCode (consomme `version.json`)

- [ ] Réglage `standardoc.coreVersion` : `"bundled" | "latest" | "<semver>"`
- [ ] Downloader de binaire : fetch `version.json`, GET du tarball correspondant à la plateforme, vérification SHA256, cache dans `globalStorageUri`
- [ ] Bascule de la cible de `binary-resolver.ts` vers le binaire téléchargé quand le réglage != `"bundled"`
- [ ] Compat check au boot : toast d'avertissement si core/ext hors de la fenêtre `min_compat`
- [ ] Surface `protocol_version` dans l'output de `standardoc --version`, parse + check ext-side
- [ ] Affordance dans l'UI des réglages pour refresh / re-download / clear cache

---

## Reporté / abandonné

- [x] ~~Templating DSL v0 (expressions markdown `{{ @doc.X }}`)~~ — abandonné au profit de la couche MDX/React (voir beta.3)
- [x] ~~Commande `materialize`~~ (écrire les virtual annotations dans la source) — punté ; peut revenir en opt-in une fois les virtual annotations atterries
- [x] ~~Binaire séparé `standardoc-server`~~ — consolidé dans les sub-commandes `standardoc`
- [x] ~~Providers Lua / Python / tree-sitter en beta.1~~ — provider Lua natif shippé en beta.2 (`full_moon`) ; Python + tree-sitter reportés post-1.0
- [x] ~~Fichier de config `.standardoc.json`~~ — remplacé par `.stdignore` + table SQLite `schema_meta`
- [x] ~~`.stdocignore`~~ — renommé en `.stdignore`
- [x] ~~`cargo install standardoc-cli` comme unique canal de distribution~~ — beta.1 ship des binaires pre-built cross-platform via GitHub Releases (`release.yml`) ; `cargo install --git` disponible pour les builds source
- [x] ~~Couche RAG de retrieval de prose (beta.2)~~ — retirée en beta.3 ; Standardoc parie sur la résolution structurelle plutôt que la similarité vectorielle
- [x] ~~DB de session handoff + compréhension projet cross-session (beta.2)~~ — extraites vers un outil voisin de session-store ; le core reste un graphe de code, pas un store de mémoire agent
- [x] ~~Surface `usage_stats` / token-savings (beta.2)~~ — retirée en beta.3

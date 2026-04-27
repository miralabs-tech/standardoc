<!--
  FICHIER AUTO-GÉNÉRÉ — NE PAS ÉDITER.
  Source: docs-src/README.fr.md
  Re-render via: ./scripts/render-docs.sh
  CI gate: .github/workflows/docs-render.yml
-->

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="branding/lockup-dark.svg">
    <source media="(prefers-color-scheme: light)" srcset="branding/lockup-light.svg">
    <img alt="standardoc" src="branding/lockup-light.svg" width="520">
  </picture>
</p>

<p align="center"><strong>La documentation source-of-truth pour du code que les agents IA peuvent réellement consommer.</strong></p>

<p align="center"><a href="README.md">English</a> · 📖 Français</p>

<p align="center">
  <a href="ABOUT.fr.md">À propos</a> ·
  <a href="QUICKSTART.fr.md">Démarrage rapide</a> ·
  <a href="docs/cli-reference.fr.md">Référence CLI</a> ·
  <a href="docs/mcp-reference.fr.md">Référence MCP</a> ·
  <a href="docs/ai-integration.fr.md">Intégration IA</a> ·
  <a href="CHANGELOG.md">Changelog</a>
</p>

---

> [!WARNING]
> **Alpha/beta — `v0.x.x`.** Je ship vite sur ce rewrite. Attends-toi à
> des releases fréquentes, des breaking changes occasionnels entre
> versions mineures, et de l'itération rapide. La surface d'API gèle
> seulement à `v1.0.0` — d'ici là, toute la toolchain reste 100% OSS
> sous FSL-1.1-MIT et le tier closed-source [Standardoc Pro](/) ne sort
> pas.
>
> Ce rewrite Rust supersède mon prototype TypeScript antérieur
> [`SUP2Ak/standardoc-cli`](https://github.com/SUP2Ak/standardoc-cli),
> sur lequel j'ai itéré perso pendant des années. Ce que tu vois ici,
> c'est la version rebuild-from-scratch avec de vrais parsers AST, un
> serveur MCP, un LSP, des annotations virtuelles, et une ambition
> beaucoup plus large.

Standardoc découple les *données structurées* (annotations dans ton code source)
de la *prose narrative* (markdown que tu écris). Il scanne n'importe quel
codebase, construit un index de tous les symboles documentables, et l'expose
via :

- Un **DSL** pour injecter des fragments de code à jour dans tes `.md` rédigés à la main
- Un **serveur LSP** pour la complétion, la navigation, les diagnostics et le rename dans ton éditeur
- Un **serveur MCP** pour que les agents (Claude Code, Cursor, Zed, Continue, …) interrogent l'index en ~100 tokens au lieu de grep+read sur 30k–100k tokens

La proposition de valeur centrale : **zéro drift** entre ce qui est dans le
code et ce qui apparaît dans la doc. Les annotations vivent à côté de leur
symbole, la prose vit en markdown, et le DSL fait le lien entre les deux.

## Installation

**Linux / macOS** :

```sh
curl -fsSL https://raw.githubusercontent.com/miralabs-tech/standardoc/main/scripts/install.sh | sh
```

**Windows (PowerShell)** :

```powershell
irm https://raw.githubusercontent.com/miralabs-tech/standardoc/main/scripts/install.ps1 | iex
```

Les deux scripts récupèrent la dernière release depuis
[GitHub Releases](https://github.com/miralabs-tech/standardoc/releases),
vérifient le checksum SHA256 et installent les binaires `standardoc` +
`standardoc-server` dans `~/.standardoc/bin/`. Pour une version spécifique :
`STANDARDOC_VERSION=v0.1.0` (variable d'environnement, fonctionne sur les deux plateformes).

**Build depuis les sources** :

```sh
git clone https://github.com/miralabs-tech/standardoc
cd standardoc
cargo build --release -p standardoc -p standardoc-server
```

Les binaires release atterrissent dans `target/release/`. Ajoute ce dossier à
ton `PATH`, ou copie `standardoc` et `standardoc-server` dans un répertoire
déjà dans le `PATH`.

**Binaires pré-compilés** : chaque release tagué publie des archives pour
`x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, `x86_64-apple-darwin`,
`aarch64-apple-darwin` et `x86_64-pc-windows-msvc` — téléchargeables depuis la
[dernière release](https://github.com/miralabs-tech/standardoc/releases/latest)
si les scripts d'install ne conviennent pas à ton setup.

## Démarrage rapide

Voir [`QUICKSTART.fr.md`](QUICKSTART.fr.md) pour un tour d'horizon en 5 minutes. En bref :

```sh
# 1. Scanner un workspace, afficher les DocBlocks canoniques en JSON
cargo run -p standardoc -- scan examples/rust-lib/src

# 2. Rendre un template markdown contre le scan
cargo run -p standardoc -- transform examples/rust-lib examples/rust-lib/docs-src/api.md

# 3. Valider les annotations + le DSL (STD001 clés dupliquées, STD004 refs cassées, …)
cargo run -p standardoc -- validate examples/rust-lib

# 4. Lancer le daemon (LSP + MCP + watcher) pour intégration éditeur / agent live
cargo run -p standardoc-server --release -- --mcp --workspace .
```

## Annoter le code source

```rust
/// Additionne deux entiers.
/// @doc calculator.add add
/// @param a i32 premier opérande
/// @param b i32 deuxième opérande
/// @returns i32 la somme
pub fn add(a: i32, b: i32) -> i32 { a + b }
```

La ligne `@doc` déclare la clé canonique et un label optionnel. `@param` et
`@returns` utilisent une convention positionnelle `<nom> <type> <description>`.
La prose au-dessus du premier `@tag` devient la description implicite.

Langages supportés out-of-the-box :

| Langage | Provider crate | Backend |
|---|---|---|
| Python | ``standardoc-lang-python`` | ``rustpython-parser`` |
| Rust | ``standardoc-lang-rust`` | ``syn`` |
| Lua | ``standardoc-lang-tree-sitter`` | ``tree-sitter`` |
| TypeScript / JavaScript | ``standardoc-lang-ts`` | ``swc`` |

**Ajoute n'importe quel autre langage sans recompiler** : dépose un fichier
JSON de config dans `.standardoc/languages/`. Voir
[`examples/dynamic-langs/`](examples/dynamic-langs/) pour des forks tree-sitter
(CfxLua, MoonScript, …) et des fallbacks regex pure.

## Écrire du markdown qui pioche dans l'index

````markdown
# `{{ @doc.calculator.add:label }}`

{{ @doc.calculator.add:description }}

```rust
{{ @doc.calculator.add:symbol.signature }}
```

## Paramètres

{{ each p in @doc.calculator.add:param }}
- **{{ p.name }}** (`{{ p.type }}`) : {{ p.description }}
{{ /each }}

**Retourne** (`{{ @doc.calculator.add:returns.type }}`) : {{ @doc.calculator.add:returns.description }}
````

Règles des clés DSL :
- `.` navigue à l'intérieur de la clé du bloc (un FQN peut contenir des points : `api.users.create`)
- `:` passe de la clé à un accesseur — soit un champ du bloc (`label`, `meta.path`,
  `symbol.signature`), soit un tag (`description`, `param[0].name`)
- `{{ each X in @doc.KEY:tag }} … {{ /each }}` itère
- `{{ each block in @docs.module(prefix) }} … {{ /each }}` itère les blocs
  d'un sous-module (ou `@docs.all` pour tout)
- `{{ if CONDITION }} … {{ else if CONDITION }} … {{ else }} … {{ /if }}`
- Les directives de bloc seules sur une ligne consomment cette ligne — pas de blanc fantôme

La référence DSL complète est exposée via le tool MCP `get_dsl_reference` —
le même contenu est servi directement aux agents et aux clients IDE.

## Ce qui est dans la boîte

### CLI (`standardoc`)
`scan`, `transform`, `emit`, `validate`, `materialize`. Opérations one-shot
sur un workspace.

### Daemon (`standardoc-server`)
Processus long-running qui expose deux protocoles simultanément :
- **LSP** (stdio) — complétion sur `@doc.…`, hover, goto-definition,
  references, workspace symbols, document outline, semantic tokens pour le
  DSL, code actions (insert squelette `@doc`, quick fixes), rename à travers
  les `.md` et les tags `@doc` source, push diagnostics à chaque rescan
- **MCP** (stdio, flag `--mcp`) — tools pour les agents :

  - ``coverage_report``
  - ``emit_llms_full``
  - ``emit_llms_txt``
  - ``emit_openapi``
  - ``emit_skill_md``
  - ``evaluate_dsl``
  - ``find_implementations``
  - ``find_references``
  - ``find_undocumented``
  - ``find_usages``
  - ``get_comments``
  - ``get_definition``
  - ``get_doc``
  - ``get_dsl_reference``
  - ``get_hover``
  - ``get_type_hierarchy``
  - ``get_watch_status``
  - ``list_collisions``
  - ``list_diagnostics``
  - ``list_docs``
  - ``render_markdown``
  - ``rescan``
  - ``resolve_symbol``
  - ``search_by_param_type``
  - ``search_by_return_type``
  - ``search_docs``
  - ``set_watch_paused``
  - ``validate_doc_syntax``

Un **watcher** intégré (debouncé + auto-pause sur les parse storms) maintient
l'index synchronisé avec le disque ; les bumps de revision push de nouveaux
diagnostics aux clients LSP sans polling.

### Validator
10 règles de lint out-of-the-box, chacune surchargeable via le `rules` du `.standardoc.json` :

| Code | Sévérité | Description |
|------|----------|-------------|
| STD001 | Error | `DocKey` dupliquée |
| STD002 | Warning | `@tag` malformé (`@doc` sans clé, `@param` sans nom, …) |
| STD003 | Warning | `@param NAME TYPE` sans description |
| STD004 | Warning | Ref DSL vers une `DocKey` qui n'existe pas |
| STD005 | Info | Bloc sans description (explicite ou implicite) |
| STD006 | Hint | Symbole public sans annotation `@doc` (inline maintenant la suggestion virtuelle quand disponible) |
| STD007 | Error | Syntaxe DSL invalide dans une page narrative |
| STD008 | Warning | `@param NAME` absent de la signature AST |
| STD012 | Warning | `@param NAME TYPE` dont le type diffère de l'AST |
| STD013 | Hint | `@doc <key>` explicite redondant — même valeur que la clé inférée depuis le FQN |

(Codes STD009–STD011 réservés.)

### Formats d'émission
`llms.txt` / `llms-full.txt` (standard de Jeremy Howard), `skill.md` (Claude
Code skills), et `OpenAPI 3.0` (depuis les tags `@route`/`@param`/`@response`).

## Setup MCP

Dépose un `.mcp.json` à la racine de ton workspace :

```json
{
  "mcpServers": {
    "standardoc": {
      "type": "stdio",
      "command": "/chemin/absolu/vers/standardoc-server",
      "args": ["--mcp", "--workspace", "/chemin/absolu/vers/ton/projet"]
    }
  }
}
```

Après avoir rebuild le binaire (`./scripts/build.sh` ou `./scripts/build.ps1`,
option `[2] prod`), démarre une nouvelle conversation Claude Code — le MCP
prend le nouveau binaire sans qu'il faille redémarrer VSCode entièrement.

## Layout du workspace

```
crates/
├── standardoc-core             # data model, DSL, index, watcher, validator, scanner
├── standardoc-lang-rust        # provider basé sur syn
├── standardoc-lang-ts          # provider basé sur swc
├── standardoc-lang-python      # provider Python AST
├── standardoc-lang-tree-sitter # provider tree-sitter générique (Lua + forks dynamiques)
├── standardoc-cli              # CLI one-shot
├── standardoc-server           # daemon LSP + MCP + Web (HTTP/SSE)
├── standardoc-web              # backend HTTP/SSE (REST API pour n'importe quel frontend)
├── standardoc-wasm             # bindings browser
└── standardoc-test-utils       # helpers de test internes
examples/                       # démos end-to-end runnables
scripts/                        # helpers de build (menu interactif : dev / prod / inspect)
```

## Configuration (`.standardoc.json`)

Optionnel. Dépose-le à la racine du workspace pour customiser :

```json
{
  "version": 2,
  "docTag": "doc",
  "hideTag": "hide",
  "discovery": {
    "exclude": ["myproject.bench.*", "myproject.dev.__*"],
    "exclude_files": ["**/*.generated.ts"],
    "virtual_annotations": "medium"
  },
  "rules": {
    "STD006": "off"
  },
  "watch": {
    "enabled": true,
    "debounceMs": 100
  }
}
```

`@hide` (ou autre, selon le `hideTag` configuré) sur un doc-comment exclut le
bloc de l'index, côté source. `discovery.exclude` fait pareil via patterns
de `DocKey`, côté config.

**Filtrage au niveau fichier** (appliqué *avant* que le scanner n'ouvre un fichier) :

- Le scanner respecte déjà le `.gitignore` du repo (donc `node_modules/`,
  `target/`, `dist/`, … sont sautés sans config).
- Pose un `.stdocignore` à côté du `.gitignore` pour des règles
  gitignore-style additionnelles dédiées à l'indexation de doc — utile quand
  tu veux des politiques différentes pour le tracking git vs l'indexation doc.
- En plus, `discovery.exclude_files` ajoute des patterns gitignore-style
  spécifiques à Standardoc directement depuis `.standardoc.json`. Utilise
  `!pattern` pour ré-inclure un fichier exclu par un pattern parent.

La distinction compte : `discovery.exclude` filtre par `DocKey` *après* le
scan (post-extraction, utile pour cacher des modules dont tu ne peux ou ne
veux pas modifier le code) ; `exclude_files` et `.stdocignore` sautent les
fichiers *avant* le parse (plus rapide, plus agressif).

## Annotations virtuelles — utilité day-1 sur n'importe quel fork

Le pain point classique des outils de doc : tu clones un projet, l'agent
n'a *rien* à mâcher, chaque question retombe sur `grep + cat`. Le pass
virtual-annotation de Standardoc ferme cette faille automatiquement.

Après l'extraction AST, chaque symbole public non annoté reçoit du contenu
virtuel `@doc`/`@param`/`@returns` synthétisé depuis les conventions de
nommage, les signatures de type, et la structure modulaire. Le contenu
synthétisé vit dans `DocBlock.virtualTags` (séparé de `tags` réels) et
`get_doc` retourne les deux — les agents voient des descriptions utiles
sans que personne ne les ait écrites.

Quelques exemples (`level: medium`, le défaut) :
- `fn new(...) -> Self` → "Creates a new `{ParentType}`."
- `is_active(&self) -> bool` → "Returns `true` if active."
- `get_user(id: u64) -> Option<User>` → "Returns the user." + `@param id`
  virtuel et `@returns Option<User>`
- `impl Display for Foo` → "Formats `Foo` for human-readable display."
- `impl From<&str> for Url` → "Converts a `&str` into a `Url`."

Contrôle de tier via `discovery.virtual_annotations` dans `.standardoc.json` :

| Level | Ce que ça couvre |
|---|---|
| `off` | Pass désactivé. MCP retourne uniquement les signatures AST. |
| `low` | Symboles publics, templates highest-confidence seulement (`new`, `is_*`, `len`, trait impls). |
| `medium` (défaut) | `low` + conventions verb-prefix + hints param-name + narrative return-type. |
| `high` | `medium` + symboles crate-privés + catégorisation module-path. |

Une fois qu'une annotation virtuelle est suffisante, promeut-la en vraie
`///` dans le source via :

```sh
standardoc materialize ./mon-projet
# Dry-run par défaut. Ajoute --apply pour vraiment éditer les fichiers.
# --confidence low|medium|high pour filtrer (défaut : medium).
```

La commande `materialize` formate le contenu virtuel en doc-comment
language-appropriate (`///` pour Rust, `---` pour Lua, `/** … */` pour TS/JS) et les
insère au-dessus de la déclaration du symbole, en préservant l'indentation.
Une fois écrites, la vraie annotation gagne — le contenu virtuel disparaît
pour ce bloc au prochain scan.

## Statut (2026-04)

**v0.1.0 release-ready** pour le pipeline core + LSP + MCP. CLI, daemon,
suite complète de tools MCP, LSP avec propagation du rename, language providers Rust/TS/Python/Lua tree-sitter — tout est live.

## Open-source vs Standardoc Pro

Standardoc est **open-core**. Deux livrables distincts :

- **Standardoc Core** *(ce repo)* — CLI, LSP, serveur MCP, tous les language
  providers, DSL, validator, plugins API, backend HTTP/SSE. Source sous
  **FSL-1.1-MIT**. Libre pour tout usage non-concurrent ; conversion en MIT
  pure deux ans après chaque release.
- **Standardoc Pro** *(séparé, post `v1.0.0`)* — l'UI web polish (navigation
  GitBook-like, composants MDX live, édition live, search, polish).
  Closed-source, achat **lifetime** unique, pas d'abonnement. Distribué
  comme un binaire qui bundle le frontend officiel. Livré séparément pour
  garder ce repo 100% OSS. **Gardé en réserve pendant le cycle `v0.x.x`**
  pour que la surface d'API se stabilise d'abord ; tout ce que tu peux
  installer aujourd'hui est OSS sous FSL-1.1-MIT.

Sans Pro, le binaire OSS `standardoc-server` expose tout programmatiquement —
tu peux construire ton propre frontend contre les endpoints `/api/*`
documentés, ou brancher n'importe quel SSG externe (Astro, Vitepress, Hugo,
…) sur `standardoc emit web --out` (mode data-only).

## Versioning & releases

Standardoc suit [SemVer 2.0](https://semver.org/spec/v2.0.0.html).

- **Releases stables** taggées `vX.Y.Z` (ex : `v0.1.0`).
- **Pré-releases** taggées `vX.Y.Z-rc.N` et flaggées comme pre-release sur GitHub.
- **Caveat pré-1.0** : tant qu'on est sous `v1.0.0`, les bumps MINOR peuvent
  inclure des breaking changes (convention SemVer pré-1.0). Les bumps PATCH
  restent backwards-compatible. À partir de `v1.0.0`, les breaking changes
  exigent un MAJOR.

Chaque release embarque :
- Binaires pré-compilés pour Linux x64/arm64, macOS x64/arm64, Windows x64
- Checksums SHA256 pour chaque archive
- Des release notes (attachées au tag) décrivant chaque changement
  user-visible depuis la précédente release

Le pipeline release est entièrement automatisé : pousser un tag `vX.Y.Z`
déclenche [`.github/workflows/release.yml`](.github/workflows/release.yml),
qui build, package, vérifie et publie la GitHub Release.

**Bumper** :

1. Mettre à jour `version` dans le `Cargo.toml` workspace (racine).
2. Commit avec `chore: release vX.Y.Z`.
3. Tag : `git tag vX.Y.Z && git push origin main vX.Y.Z`.
4. Écrire les release notes directement sur la page du tag une fois que
   la pipeline de build a produit les archives.

## Contribuer

Les contributions sont les bienvenues — bug reports, fixes, language providers,
règles de validator, améliorations de doc, tout est utile.

**Setup** :

```sh
git clone https://github.com/miralabs-tech/standardoc
cd standardoc
# Installer la toolchain Rust (1.89+) — voir rust-toolchain.toml
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

Les helpers de build interactifs dans `scripts/build.{sh,ps1}` couvrent le
cycle de dev local courant (`[1] dev` build dans `target-dev/` pour
itération parallel-safe, `[2] prod` kill les serveurs et rebuild dans `target/`).

**Workflow** :

1. Ouvrir une issue d'abord pour les changements non-triviaux — ça t'évite
   d'écrire du code qui rentre en conflit avec du travail en cours ou une
   décision de roadmap.
2. Fork → branche → PR contre `main`. Garde les PR focalisées sur une seule chose.
3. Référence l'issue (`Fixes #123`) et décris le changement dans le body.
4. La CI doit être verte : `fmt`, `clippy --all-targets -D warnings`, `test`, `docs`.
5. Un mainteneur review et merge (stratégie squash, donc le nombre de commits
   dans la PR n'a pas d'importance — c'est le message au merge qui compte).

**Conventions** :

- **Commits** : format Conventional Commits (`feat:`, `fix:`, `chore:`,
  `docs:`, `refactor:`, `test:`, `perf:`, etc.). Utilisé pour générer les
  entrées de changelog et faire respecter la sémantique des releases.
- **Noms de branches** : `feat/short-description`, `fix/issue-123`, etc.
- **Pas de commentaires par défaut** : le code doit s'auto-documenter via du
  naming clair. Ajoute un commentaire seulement quand le *pourquoi* est
  non-évident — et écris-le **en anglais uniquement**.
- **Pas de nouvelle dépendance sans discussion** : ouvre une issue d'abord.
- **Doc multi-langue** : les fichiers `.md` user-facing principaux ont une variante
  `.fr.md`. Si tu modifies `README.md`, mets à jour `README.fr.md` aussi (ou
  ouvre une issue pour signaler le décalage si tu n'es pas francophone).
  L'EN est la source de vérité ; le FR suit.

**Ajouter un language provider** :

Implémente le trait `LanguageProvider` de `standardoc-core`. Les providers
existants (`standardoc-lang-rust`, `standardoc-lang-ts`,
`standardoc-lang-python`, `standardoc-lang-tree-sitter`) sont de bonnes
références. Pour les langages exotiques sans parser natif existant, une
grammaire tree-sitter est en général le chemin de moindre résistance — voir
[`examples/dynamic-langs/`](examples/dynamic-langs/) pour l'approche JSON
config chargée au runtime.

**Code of Conduct** : être respectueux, présomption de bonne foi, pas de
harcèlement. En attendant qu'un CoC formel soit publié, le
[Contributor Covenant](https://www.contributor-covenant.org/) s'applique
par défaut — signaler les problèmes aux mainteneurs via l'email dans `Cargo.toml`.

**Sécurité** : ne pas ouvrir d'issues publiques pour les vulnérabilités de
sécurité. Email directement le mainteneur (voir `authors` dans `Cargo.toml`)
en attendant qu'une `SECURITY.md` soit publiée.

## Soutenir le projet

Je suis le seul mainteneur de Standardoc Core, et j'y bosse en parallèle
d'un boulot. S'il te fait gagner du temps, deux façons de redonner :

- **OpenCollective** — donations récurrentes ou one-time, soutiennent le
  développement core, les language providers et les règles de validator.
  *(Profil en cours de setup — le lien sera ajouté en v0.1.1.)*
- **[Standardoc Pro](/)** — achète une licence lifetime pour l'UI web polish
  *(disponible post `v1.0.0` — gardé en réserve pendant le cycle `v0.x.x`
  pour que l'API se stabilise d'abord)*. Le revenu direct finance tout
  l'écosystème, y compris le Core OSS que tu utilises là.

Pour du sponsoring commercial, des language providers custom ou des contrats
de support payant : contact via l'email dans `Cargo.toml` `authors`.

## Licence

[**FSL-1.1-MIT**](LICENSE) — Functional Source License v1.1 avec future
license MIT. Tu peux utiliser, modifier et redistribuer Standardoc Core pour
tout usage **sauf** offrir un produit ou service concurrent qui se substitue
à Standardoc lui-même. Deux ans après chaque release, cette release passe
automatiquement en MIT pure.

Pourquoi FSL : protège contre les offres concurrentes directes (le pattern
"open-and-pillage") sans verrouiller le core pour les utilisateurs honnêtes.
Adoptée par Sentry, CodeCrafters, Keygen.

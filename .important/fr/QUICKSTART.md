# Démarrage rapide

← [README](README.md) · [Roadmap](TODO-LIST.md)

> **⚠️ Archivé (2026-09-17).** Standardoc est archivé sur **v1.0.0-beta.2** ;
> le [README](README.md) explique pourquoi. Cette page décrit le source
> `main` (beta.3, jamais publiée). Le binaire et l'extension beta.2 publiés
> n'ont **ni** `standardoc init`, `mcp --connect`, `self-update`,
> `sxd-preview`, **ni** `standardoc.sxd` — beta.2 utilise `.stdignore` et le
> flow d'init de l'extension. Rien ici ne sera mis à jour.

De zéro à un workspace indexé que ton agent peut requêter — ~5 minutes.

---

## 1. Installer

**VSCode / Cursor** — cherche **Standardoc** dans le Marketplace ou Open VSX.
À la première activation, il propose de télécharger le binaire natif de ta
plateforme (vérifié SHA256) — accepte.

Pas de VSCode ? Saute au [§5](#5-sans-vscode).

## 2. Initialiser le workspace

Ouvre un projet. Standardoc demande :

> **Initialize this workspace?** — [Initialize] · [Skip] · [Never for this workspace] · [Never (any workspace)]

Clique **Initialize**. Il écrit, de façon idempotente :

- **`.mcp.json`** — enregistre Standardoc comme serveur MCP (HTTP sur `127.0.0.1`, l'URL que le daemon annonce) pour que ton agent l'atteigne.
- **`.claude/skills/standardoc/SKILL.md`** — enseigne le graphe à l'agent (MCP-first, le flow `find → context → body`).
- **`.claude/settings.json`** — les hooks MCP-first (voir §4).

…puis spawn le daemon et cold-start-indexe le workspace (quelques secondes).
Un watcher garde l'index live pendant que tu édites, et un item de status bar
montre l'état du daemon + les actions courantes.

> `.mcp.json` porte des chemins machine-absolus — ajoute-le à `.gitignore` si tu collabores.

## 3. `standardoc.sxd` — la config du workspace

Au premier index, Standardoc seede **`standardoc.sxd`** à la racine (en y
fondant un éventuel `.stdignore` legacy, sauvegardé). C'est la source de
vérité unique de ce qui est indexé :

````sxd
version "0.1.0"

ignore {
  patterns ```
.git/
node_modules/
target/
dist/
```
}

# Optionnel. Avec au moins un bloc `project`, la détection mécanique
# cargo/npm/lua est court-circuitée et SEULS ces paths sont indexés :
project "api" {
  label "API"
  paths ["crates/api" "crates/shared"]
}
````

Édite-le librement ; le ré-index prend les changements. Blocs : `ignore`,
`project` / `group`, `mcp`, `viz`. Sans bloc `project`, Standardoc
auto-détecte les projets cargo / npm / lua comme avant. N'écris pas de
`mcp { port … }` sauf besoin d'un port fixe : le port du daemon MCP doit
différer du port du proxy de l'extension (`standardoc.proxyPort`, défaut
`7700`), et une version précédente de cette page te disait de les mettre
égaux.

## 4. Utiliser

Pose des questions normales à ton agent :

> *« Où est `parse_workspace` défini ? Qui l'appelle ? »*

Il lit la skill au boot et passe MCP-first — `find_symbol` + `get_context`
au lieu de grep. Testé avec Claude Code uniquement ; les autres clients MCP
n'ont jamais été exercés. Mesuré sur ce repo, le chemin MCP n'a pas
économisé de tokens face à grep (bandeau du README).

Pour **Claude Code**, l'init installe aussi quatre hooks
`.claude/settings.json` qui *l'imposent*. Ils gênent plus qu'ils n'aident —
le hook deny cale son sentinel sur le dossier courant, pas sur la
conversation, et bloque du travail sans rapport — donc envisage de t'en
passer :

- **UserPromptSubmit** — rappel d'une ligne des tools MCP.
- **PreToolUse** *(mark)* — se déclenche sur tout appel `mcp__standardoc__*` ; marque la session.
- **PreToolUse** *(check)* — **refuse** `Bash` / `Read` / `Grep` / `Glob` tant que l'agent n'a pas utilisé Standardoc dans ce chat.
- **SessionStart** *(reset)* — wipe le marqueur pour que chaque chat reparte strict.

Un autre agent ? Câble l'équivalent via `standardoc claude pre-tool-hook --mode {mark,check,reset}`.

## 5. Sans VSCode

```sh
cargo install --git https://github.com/miralabs-tech/standardoc standardoc-cli
standardoc init <workspace>   # skill + hooks MCP-first + AGENTS.md + .mcp.json
```

`init` écrit un `.mcp.json` qui lance `standardoc mcp --connect` — un pont
léger qui garde un daemon vivant et watcher-backed pour le workspace. Ton
agent a maintenant le graphe. Pour piloter les daemons toi-même :

```sh
standardoc lsp <ws>                  # writer principal (tient le fs lock)
standardoc mcp <ws> --http <port>    # MCP via HTTP/SSE (multi-client)
standardoc mcp <ws> --readonly       # MCP via stdio (un client)
```

## 6. Sous-commandes utiles

```sh
standardoc index <ws>                   # index one-shot
standardoc rescan <ws>                  # rebuild from scratch
standardoc query <ws> ...               # query CLI (find / context / body)
standardoc sxd-preview <ws> <pattern>   # prévisualise ce que l'ignore .sxd matche
standardoc self-update                  # met à jour le binaire en place
```

Le jeu complet de tools MCP vit dans le `SKILL.md` auto-généré — l'agent le
lit, pas besoin de le mémoriser côté humain.

---

← [README](README.md) · [Roadmap](TODO-LIST.md)

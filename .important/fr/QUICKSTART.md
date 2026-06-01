# Démarrage rapide

← [README](README.md) · [Roadmap](TODO-LIST.md)

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

- **`.mcp.json`** — enregistre Standardoc comme serveur MCP (HTTP, `127.0.0.1:7700`) pour que ton agent l'atteigne.
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

mcp { port 7700 }   # port du daemon MCP  (défaut 7700)
viz { port 3000 }   # port du graph-viz   (défaut 3000)
````

Édite-le librement ; le ré-index prend les changements. Blocs : `ignore`,
`project` / `group`, `mcp`, `viz`. Sans bloc `project`, Standardoc
auto-détecte les projets cargo / npm / lua comme avant.

## 4. Utiliser

Pose des questions normales à ton agent :

> *« Où est `parse_workspace` défini ? Qui l'appelle ? »*

Il lit la skill au boot et passe MCP-first — `find_symbol` + `get_context`
au lieu de grep. **~100 tokens, pas 30k.** Claude Code, Cursor, Continue,
Copilot, n'importe quel client MCP.

Pour **Claude Code**, l'init installe aussi quatre hooks
`.claude/settings.json` qui *l'imposent* :

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

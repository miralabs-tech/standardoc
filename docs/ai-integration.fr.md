# Intégration IA — bonnes pratiques

[English](ai-integration.md) · 📖 Français

Ce guide explique comment tirer le maximum de Standardoc avec des assistants IA de coding : **pourquoi MCP-first compte**, comment **garder les long threads productifs**, des **templates de system prompt** prêts à l'emploi, et **l'installation par IDE** pour Claude Code, Cursor, Zed, Continue, et n'importe quel autre host MCP.

---

## Pourquoi MCP-first compte

Quand un agent IA répond à "qu'est-ce que la fonction `X` prend en input ?", il a deux chemins :

1. **Sans MCP** — `grep -r "fn X"`, puis `cat src/foo.rs`, puis devine. Coût : **30k–100k tokens** par question (lectures complètes de fichiers, bruit de faux positifs grep, backtracking).
2. **Avec MCP Standardoc** — appel `get_doc("module.X")`. Coût : **~100 tokens**. Retourne directement la signature canonique, les types des paramètres, le type de retour, la description et les tags.

C'est une **réduction 100x à 1000x de tokens**, sur chaque question, chaque conversation, chaque projet.

Au-delà du coût, MCP-first change la précision :
- L'agent lit la signature **actuelle**, pas ce qu'il se rappelle d'un grep stale
- Les cross-références (`find_usages`, `find_implementations`, `get_type_hierarchy`) remplacent le raisonnement flou
- Les diagnostics (`list_diagnostics`, `validate_doc_syntax`) attrapent les erreurs de l'agent avant qu'elles ne shippent

Le piège : les agents ne choisissent pas naturellement MCP en premier. Sans instruction explicite, la plupart retombent sur les habitudes grep + read entraînées sur des années de données pré-MCP. C'est là que les **templates de system prompt** ci-dessous interviennent.

> **Utilité day-1 sur un fork.** Même si aucun humain n'a jamais écrit
> `@doc` sur le codebase, le pass virtual-annotation de Standardoc
> synthétise du contenu `@doc`/`@param`/`@returns` virtuel depuis les
> conventions de nommage, signatures de type, et structure modulaire —
> `get_doc("module.X")` retourne des descriptions utiles dès le premier
> scan. Voir "Annotations virtuelles" dans le README pour la liste
> complète des heuristiques.

---

## Hygiène long thread — le pattern checkpoint

Les context windows des LLM s'agrandissent chaque année, mais le **prompt caching** a un TTL dur de 5 minutes et **l'attention de l'agent dérive** avec la longueur de la conversation. Passé ~20 échanges substantiels, tu observes typiquement :

- L'agent oublie les décisions verrouillées plus tôt ("on utilise TypeScript ou Rust déjà ?")
- Il ré-explique des trucs déjà discutés
- Le coût par tour grimpe même quand tu caches
- Des hallucinations subtiles s'insèrent (signatures dont il "se souvient" à tort)

Le fix c'est le **checkpointing explicite** — à ~20 échanges, écris un `SESSION-CHECKPOINT.md` à la racine du projet qui résume :

```markdown
# Session checkpoint — YYYY-MM-DD

## Livré dans cette session
- (features concrètes, fichiers, décisions)

## État actuel
- (ce qui marche, ce qui est en attente, état du build)

## Décisions verrouillées
- (choix d'archi qui ne devraient PAS être re-litigés)

## TODO (prochaine session)
1. (prochaines étapes concrètes)
```

Puis démarre une conversation fresh avec **ce fichier comme seul contexte**. Le nouveau thread démarre avec la même connaissance partagée mais avec un budget d'attention complet.

Ce pattern est documenté dans les [instructions globales Claude Code de l'utilisateur](https://github.com/miralabs-tech/standardoc) et marche aussi bien dans Cursor, Zed, Continue. L'UI Standardoc Pro proposera à terme une suggestion "checkpoint" automatique quand elle détecte des longs threads — en attendant, la discipline est sur toi.

---

## Templates de system prompt

Deux saveurs. Choisis selon à quel point tu veux forcer MCP-first.

### Normal — défaut recommandé (Cursor, Zed, Continue, …)

> Les templates ci-dessous sont en anglais à dessein — ils sont destinés à être collés tels quels dans le system prompt d'un agent IA.

```
# MCP/LSP First (Normal)

## Objective
Use MCP/LSP as the default path for code understanding and navigation.

## Default behavior
Before using raw file search/read:
1. Use MCP/LSP tools first for:
   - symbol discovery
   - definition lookup
   - references/usages
   - diagnostics
   - high-level architecture mapping
2. Prefer semantic/symbolic results over text grep when both are possible.

## Fallback policy
Use non-MCP exploration only if:
- MCP/LSP cannot answer, or
- MCP data is incomplete/outdated for the requested target.

When falling back, briefly state:
- why MCP was insufficient
- which fallback method is used
- what result is expected

## Editing policy
For code changes:
- Use MCP/LSP first to identify the exact symbols/files to edit.
- Then apply minimal file edits.
- Re-check impact via MCP/LSP diagnostics/references when available.
```

### Strict — recommandé pour Cursor & Claude Code sur du sérieux

```
# MCP/LSP First (Strict)

## Hard rule
For discovery/analysis tasks, MCP/LSP MUST be used first.
Do not start with raw file search tools.

## Mandatory sequence
1) MCP/LSP discovery
2) MCP/LSP symbol/reference/diagnostic checks
3) Only then, if needed, file-level fallback
4) Edit
5) MCP/LSP re-validation

## Allowed fallback (exception only)
Fallback to non-MCP exploration is allowed only if:
- MCP/LSP has no capability for the task, or
- MCP/LSP returns insufficient/noisy data that blocks progress.

Before fallback, explicitly state:
- "MCP/LSP insufficient because: <reason>"
- "Fallback method: <method>"
- "Scope: <minimal scope>"

## If user says "MCP only"
Use MCP/LSP exclusively for analysis.
File tools may be used only for final patch application.
```

Le gros gain de ce template c'est le **fallback transparent** : l'agent ne peut pas abandonner silencieusement MCP pour grep — il doit déclarer *pourquoi* MCP n'a pas suffi et ce qu'il fait à la place. Ça te donne, à toi utilisateur, une boucle de feedback :
- Si tu vois la même raison de fallback se répéter ("MCP n'a pas retourné les descriptions pour X"), c'est un vrai signal qu'il te manque des annotations `@doc` sur ces symboles — fix la donnée, pas le prompt.
- Si l'agent fallback pour des raisons légitimes (ex : besoin de lire une page `.md` non indexée), tu comprends pourquoi au lieu de supposer que l'agent est paresseux.

Utilise le template **Strict** quand :
- Le codebase est bien indexé par Standardoc (la plupart des symboles publics annotés)
- Tu fais du travail d'implémentation où une mauvaise info coûte du temps réel
- Tu veux une discipline de niveau CI + une trace d'audit visible de chaque fallback

Utilise le template **Normal** quand :
- Le codebase est partiellement indexé (beaucoup de symboles pas encore `@doc`'d)
- Tu fais de l'exploration / brainstorming où la flexibilité aide
- Tu ne veux pas que l'agent stale sur le cérémonial du fallback transparent

---

## Setup par IDE

Le setup c'est le même snippet `.mcp.json` partout — seuls l'emplacement et le mécanisme de découverte diffèrent.

### Claude Code

**Par projet** — dépose `.mcp.json` à la racine du workspace :

```json
{
  "mcpServers": {
    "standardoc": {
      "type": "stdio",
      "command": "/chemin/absolu/vers/standardoc-server",
      "args": ["--mcp", "--workspace", "${workspaceFolder}"]
    }
  }
}
```

Redémarre Claude Code (ou lance `/mcp` pour re-découvrir) et les tools MCP Standardoc deviennent disponibles.

**System prompt global** — ajoute le template Strict à ton `~/.claude/CLAUDE.md` global :

```markdown
## Tool Hierarchy (mandatory)

When exploring or understanding code, always follow this order:

1. **MCP tools** — primary source of truth (indexed, structured, fast)
2. **LSP tools** — symbol resolution, definitions, references
3. **Read / Grep** — last resort only, and only when MCP/LSP return nothing

Never reach for `Read` or `Grep` on source files when an MCP tool can answer
the question. If MCP returns no result, say so explicitly before falling back.
```

**Checkpoint long thread** — ajoute au même `CLAUDE.md` :

```markdown
## Long Thread Management

When a conversation reaches ~20 significant exchanges, write a
`SESSION-CHECKPOINT.md` at the project root summarizing what was shipped,
current state, locked decisions, and what remains. Then suggest starting a
new thread with that file as the only context.
```

### Cursor

**Par projet** — le même `.mcp.json` à la racine du workspace marche depuis Cursor 0.42+.

**Project rules** — dépose `.cursorrules` à la racine du workspace (Cursor lit ça à chaque chat). Les deux templates marchent ; **Strict est recommandé dès que le daemon tourne et que l'index est peuplé** — ce qui arrive au boot, même sans aucune annotation `@doc` (l'AST te donne déjà signatures, params, types de retour ; les annotations enrichissent le payload mais ne sont pas un prérequis). Les déclarations de fallback transparent te donnent alors un signal utile quand MCP ne suffit pas — typiquement quand les descriptions manquent parce qu'un symbole n'a pas encore été annoté.

```
# Project rules

When exploring or modifying code, prefer MCP tools over raw file reads :
[colle le template Normal ou Strict ici]
```

### Zed

Dans `~/.config/zed/settings.json` (ou workspace-level `.zed/settings.json`) :

```json
{
  "context_servers": {
    "standardoc": {
      "command": {
        "path": "/chemin/absolu/vers/standardoc-server",
        "args": ["--mcp", "--workspace", "/chemin/absolu/vers/ton/projet"]
      }
    }
  }
}
```

Le context server Standardoc apparaît dans le panneau Assistant comme `@standardoc`.

### Continue

Édite `~/.continue/config.json` :

```json
{
  "experimental": {
    "modelContextProtocolServers": [
      {
        "transport": {
          "type": "stdio",
          "command": "/chemin/absolu/vers/standardoc-server",
          "args": ["--mcp", "--workspace", "/chemin/absolu/vers/ton/projet"]
        }
      }
    ]
  }
}
```

### Host MCP générique

N'importe quel client MCP 1.0-compatible marche — Standardoc est un serveur stdio vanilla. Pointe ton client vers :

- **command** : `standardoc-server` (ou full path si pas dans `$PATH`)
- **args** : `["--mcp", "--workspace", "<chemin-absolu-vers-projet>"]`
- **transport** : stdio (JSON-RPC 2.0)

La version du protocole est `2025-06-18`. Les tools s'auto-découvrent via la méthode MCP standard `tools/list`. Voir la [référence MCP](mcp-reference.fr.md) pour la surface complète.

---

## Vérification — MCP est-il branché correctement ?

Une fois configuré, demande à l'agent : **"Liste les blocs de documentation de ce workspace en utilisant le MCP standardoc."** Un setup qui marche retourne une liste structurée. Un setup cassé fallback sur du grep dans les fichiers `.md` (signal révélateur).

Autres checks rapides :
- Demande : *"Quels tools MCP tu vois pour standardoc ?"* — l'agent devrait énumérer `list_docs`, `get_doc`, `find_usages`, etc. S'il dit "je ne vois pas de tools standardoc", le host n'a pas chargé les schémas (dans Claude Code, c'est l'étape `ToolSearch` pour les tools deferred — l'agent doit l'appeler avant que les tools MCP ne soient invokables).
- Demande : *"Que `standardoc-server` expose comme tools MCP ?"* — devrait appeler `list_docs` ou similaire, pas lire le source.
- Demande : *"Trouve chaque usage de `DocBlock` dans le codebase."* — devrait appeler `find_usages`, pas `grep`.
- Si l'agent a grep, le system prompt ne force pas MCP-first — relis la section **Allowed fallback** ci-dessus.

---

## Pro tip — dogfooder Standardoc sur Standardoc lui-même

Standardoc s'indexe lui-même. Si tu checkout [`miralabs-tech/standardoc`](https://github.com/miralabs-tech/standardoc) et que tu pointes un agent MCP-aware dessus, tu peux demander :

- *"Que fait `find_implementations` exactement ?"* — récupère la doc complète du tool MCP en interrogeant l'index live, pas en lisant le README.
- *"Trouve chaque endroit où on appelle `scan_and_extract`."* — `find_usages` pointe les emplacements exacts, l'agent n'ouvre jamais un fichier à l'aveugle.
- *"Montre-moi tous les codes validator (STD###) et ce que chacun attrape."* — `search_docs` + `get_doc` retourne de l'info précise et structurée.

C'est le test le plus rigoureux du pattern. Si l'agent reste MCP-first sur le codebase de Standardoc lui-même (un workspace Rust complexe), il marchera sur le tien.

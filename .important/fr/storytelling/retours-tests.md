# Retours de tests

[English](../../en/storytelling/test-feedback.md) · 📖 Français

[← Philosophie](philosophy.md) · [Vision court terme](vision-court-terme.md) · [Vision moyen terme](vision-moyen-terme.md) · [Vision long terme](vision-long-terme.md) · [Remarques](remarques.md)

> Ce document rassemble les **observations honnêtes** issues du dogfood :
> ce qu'on a testé, ce qu'on a abandonné, et les surprises qu'on a vues
> en utilisant Standardoc sur des cas réels.

---

## Calibration et matching d'agent

Pendant la phase **v0.1.0-rc** (prototype) puis sur tout le cycle
beta.1 → beta.2, on a testé Standardoc avec plusieurs agents IA
disponibles à l'époque. **Les comportements observés sont très
inégaux selon l'agent.**

### Les agents qui ne matchent pas

Trois patterns récurrents observés sur des agents qui ne s'alignent
pas naturellement avec l'architecture de Standardoc :

- **Shortcut vers `grep + read`** — l'agent connaît MCP, mais dès que
  la tâche se complique, il bascule vers `Bash` / `Grep` / `Read` par
  réflexe. Sans hook PreToolUse pour le bloquer, le protocole
  MCP-first reste une bonne intention non-observable.
- **Ignore le `routing_hint`** — quand un agent appelle
  `get_context(fqdn, depth=2)` sans avoir fait `depth=1` récemment
  (dans les 5 dernières minutes) sur le même FQDN, la réponse
  contient un `routing_hint` correctif qui rappelle de mapper
  d'abord en `depth=1` avant de driller en `depth=2`. C'est le
  seul cas où le hint apparaît — c'est un signal de correction
  ponctuel, pas un système de guidage général. Certains agents
  l'ignorent et continuent en `depth=2` d'office, résultat :
  drill complet là où un map peu coûteux aurait suffi à éliminer
  80% des voisins.
- **Exploration non-structurée** — l'agent consomme 100% du budget
  en recherches itératives sans capitaliser. Pas de réutilisation
  des symboles déjà visités, pas de `check_stale` pour réutiliser
  un cache.

### L'agent qui matche le mieux

**[Claude Code](https://www.anthropic.com/claude-code) en mode Opus**,
fenêtre 1M tokens, avec un tier d'effort qui varie selon la tâche
(medium / high / extra-high — jamais figé). C'est sur cet agent que
la calibration de Standardoc est faite :

- Hooks PreToolUse / SessionStart qui forcent le MCP-first guardrail
- `routing_hint` du `get_context` respecté en pratique (Opus mappe
  en `depth=1` avant de driller en `depth=2` au lieu de subir le
  correctif)
- Knobs `signature_only` / `strip_attrs` de `get_body` utilisés
  spontanément quand le budget de contexte se tend

Les autres agents fonctionnent, mais sans la même fluidité, et avec
des écarts de consommation qu'on n'observe pas avec Opus.

### Quelques estimations dogfood

Standardoc indexant lui-même, sessions Claude Code Opus : au début, la
majorité du quota 5h partait en exploration redondante (grep, re-lecture,
re-découverte de structures déjà visitées) ; aujourd'hui la même fenêtre
tient de l'ordre de deux sessions de travail. Estimations dogfood
grossières, pas des benchmarks contrôlés — elles supposent un dev qui
maîtrise son code et applique la discipline 3-phase.

### Ce qui a vraiment impressionné en dogfood

Au-delà des chiffres, l'observation qualitative la plus marquante :
**le code généré par l'agent break rarement, et respecte
spontanément l'architecture et le style du projet**. Les conventions
tacites — nommage, granularité des modules, style d'erreur,
séparation des responsabilités IR / storage / query / providers —
sont reprises par l'agent sans qu'on ait à les lui ré-expliquer à
chaque tâche.

C'est exactement ce que doit produire un graphe partagé couplé à
une discipline encodée : **un agent qui se conforme au projet, pas
qui l'écrase.**

**Mais ça ne dispense pas de la review.** Un projet maintenable
exige que le dev **connaisse son code et le comprenne** — pas
seulement qu'un agent puisse en produire qui passe les tests. La
review reste obligatoire à chaque PR, à chaque chantier. Standardoc
rend la review *plus rapide et plus fiable* (l'agent suit les
conventions, donc moins de surprises côté reviewer) ; il ne la
*remplace pas*.

### Disclaimer : calibration tripartite

**On ne garantit pas que ton agent va matcher de la même façon.**
Standardoc fournit l'infrastructure (graphe + MCP + discipline) ;
si ton agent ne respecte pas le protocole MCP-first ou ignore les
correctifs du `routing_hint` quand il viole le pacing `depth=1 →
depth=2` de `get_context`, tu n'auras qu'une partie des gains. **La
calibration est tripartite** : infrastructure + agent + opérateur.

#### Ce que l'infrastructure fournit déjà

- **Skill template auto-généré** (`SKILL.md` écrit dans
  `.claude/skills/standardoc/`) — enseigne à l'agent la tool fallback
  hierarchy (Standardoc → LSP → Read / Grep), le 3-phase protocol
  (Explore → Cibler → Drill), les edge kinds et la couverture langages.
  L'agent qui le lit au boot n'a pas à deviner la mécanique.
- **Hooks MCP-first** (côté Claude Code) — PreToolUse bloque
  `Bash` / `Read` / `Grep` / `Glob` tant qu'aucun tool Standardoc
  n'a été appelé dans la session ; SessionStart wipe le sentinel
  à chaque nouveau chat. Discipline observable, pas bonne
  intention.

#### Ce qui reste à charge de l'opérateur

Trois responsabilités humaines non-déléguables :

1. **Coupler son agent à Standardoc.** Installer l'extension
   VSCode (qui génère le skill et écrit `.mcp.json` automatiquement
   via l'init opt-in flow) ou wirer `.mcp.json` à la main pour un
   autre client MCP. Si l'agent ne voit pas Standardoc, il ne
   l'utilise pas.
2. **Architecturer son projet avec un minimum de cohérence.**
   Standardoc indexe ce qui existe — si la codebase est un
   spaghetti sans modules clairs, le graphe reflète le spaghetti.
   L'outil amplifie ce que l'opérateur cultive ; il ne le remplace
   pas.
3. **Indiquer à l'agent quand et comment utiliser les outils.**
   Le skill template enseigne la mécanique générale ; mais pour
   une tâche donnée, c'est à l'opérateur de dire *"commence par
   `find_symbol`"*, *"vérifie d'abord qui appelle X"*, *"ne grep
   pas ici"* — surtout en début de calibration, avant que l'agent
   ait intégré les habitudes de pacing par lui-même.

Si ton agent par défaut est mauvais sur ce protocole, l'option qui
marche en pratique est : (a) lui forcer le MCP-first via les hooks
Claude Code (guardrail), et (b) le laisser absorber sur 2-3 tâches les
correctifs `routing_hint` quand il saute le pacing `depth=1 → depth=2`
de `get_context`.

---

## Ce qu'on a abandonné

Plusieurs pistes initiales ont été tuées ou repoussées — le DSL templating
`{{ @doc.X }}`, la commande `materialize`, un binaire `standardoc-server`
séparé, la config `.standardoc.json`, `cargo install` comme unique canal,
et les couches RAG / sessions (en beta.3). La liste complète avec les
raisons vit dans
[TODO-LIST → Reporté / abandonné](../TODO-LIST.md#reporté--abandonné).

---

## Pour aller plus loin

- **[← Philosophie](philosophy.md)** — les 5 principes
  system-thinking et l'éthique de construction
- **[Vision court terme](vision-court-terme.md)** — beta.2 et la
  phase de stabilisation
- **[Vision moyen terme](vision-moyen-terme.md)** — beta.3 et 1.0
- **[Vision long terme](vision-long-terme.md)** — UST + Lua plugin
  layer post-1.0
- **[Remarques](remarques.md)** — observations dogfood, décisions
  lockées, apprentissages
- **[TODO-LIST](../TODO-LIST.md)** — checkboxes exhaustives par
  milestone

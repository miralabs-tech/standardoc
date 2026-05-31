# Vision court terme — beta.2 → 1.0

[English](../../en/storytelling/vision-short-term.md) · 📖 Français

[← Philosophie](philosophy.md) · [Vision moyen terme →](vision-moyen-terme.md) · [Vision long terme →](vision-long-terme.md)

> Ce document est le **narratif** derrière les milestones court-terme.
> La liste exhaustive des features par milestone vit dans
> [`TODO-LIST.md`](../TODO-LIST.md) — c'est elle qui bouge, pas ce doc.

---

## Où on en est

Standardoc sort du **v1.0.0-beta.1**. Cette fondation comportait AST
direct Rust + TS, IR canonique cross-langue, SQLite + FTS5 + file
watcher, daemons LSP/MCP, extension VSCode, et l'infra distribution
(releases cross-platform, version.json manifest, FSL-1.1-MIT).

beta.1 est **stable**. Ce qui ship sur `main` (Rust + TS, 2 MCP tools,
LSP, ext VSCode) est utilisable en prod et le restera — on n'arrache
rien rétroactivement.

beta.2 n'est pas une release de "nouvelles features qu'on veut vendre".
**C'est une release de maturité** — la fondation de beta.1 mise à
l'épreuve de l'usage réel sur 3 cibles dogfood en parallèle, et
raffinée jusqu'à ce qu'elle tienne sous des charges qu'on n'avait pas
imaginées au départ.

---

## Les cibles dogfood pendant la phase beta.1 → beta.2

- **Standardoc lui-même** — indexé dans sa propre CI ; si un PR casse
  l'IR ou la surface MCP, la CI le détecte avant le merge. Si l'outil
  n'est pas utilisable sur lui-même, il ne l'est sur rien.
- **Deux autres projets** — un build engine polyglotte et un langage
  maison (Rust + C). Ils ont stressé les knobs `get_body` (`strip_attrs`,
  `signature_only`) sur des handlers lourds, et validé le pattern
  "monorepo multi-langues avec une partie hors-graphe" : l'agent utilise
  la partie indexée et bascule en `Read` documenté sur le reste, sans
  confusion.

---

## Le pivot de scope entre le plan original et la réalité

Le plan original pour beta.2 était : **doc rendering layer + CLI
self-management**. Sur le papier, c'était cohérent — remplacer le
DSL templating tué en beta.1, et rendre `standardoc` auto-suffisant
sans VSCode.

En pratique, rien de ce qui était prévu n'a été construit dans cette
phase. **Pas par procrastination — par révélation de besoins plus
prioritaires en dogfood réel** : surface MCP trop pauvre pour 90% des
flows agent, sessions qui s'évaporent entre chats, agents qui shortcut
vers grep dès qu'ils peuvent, stdio MCP limité à 1 client à la fois,
prose adjacente inaccessible au graphe, langages au-delà de Rust+TS
manquants, daemon pas assez résilient sous orchestration réelle.

Le scope a donc été ré-aligné sur ce qui a effectivement émergé :
**hardening + MCP surface refinement**. Le doc rendering et le CLI
self-managed sont passés en beta.3.

**Pas par abandon — par priorité différente.** On ne ship pas une
couche rendering si la couche en dessous est encore raffinable.

---

## beta.2 — ce que ça représente vraiment

Trois angles fondamentaux qui marquent la phase :

### 1. La surface MCP n'est plus un placeholder, c'est un toolkit

On passe d'une surface day-1 (2 tools) à une surface utilisable en
production sous des flows agent réels. Le critère n'est pas le nombre
de tools — c'est qu'**aucun trou observé en dogfood ne reste sans
réponse côté API**. Quand l'agent doit faire un audit cross-module,
quand il a besoin d'une lecture compacte d'un handler, quand il tape
un FQDN approximatif, quand il veut vérifier que sa connaissance d'un
symbole est encore fraîche — il y a un tool pour ça. Et quand l'agent
brûle du contexte en sautant le pacing recommandé (`get_context(depth=2)`
sans `depth=1` préalable), le serveur renvoie un `routing_hint`
correctif au lieu de le laisser continuer silencieusement.

### 2. L'architecture multi-frontend / multi-backend a quitté le whiteboard

beta.1 avait LSP + MCP stdio. beta.2 valide en pratique : MCP HTTP/SSE
multi-client, langages providers étendus (Lua, Vue, Svelte). L'enjeu
n'est pas chaque pièce individuellement — c'est que **le tout reste
cohérent**, sans qu'aucune surface ne corrompe l'état de l'autre.

C'est validé maintenant. Plus en théorie.

### 3. La discipline est devenue une feature, pas une convention

Le **MCP-first guardrail** transforme une bonne intention ("l'agent
devrait utiliser MCP en premier") en règle observable du système, via
les hooks Claude Code (PreToolUse + SessionStart + mark sentinel).
Plus de "l'agent devrait" — l'agent **doit** ou il est bloqué. Et le
blocage est observable (deny avec message structuré), pas silencieux.

C'est la première brique d'une approche plus large : encoder les
patterns de bon comportement dans le système, pas dans les bons
sentiments. Les autres briques (routing_hint, daemon-side
enforcement) suivent la même logique.

→ [Détails exhaustifs des features beta.2 dans TODO-LIST](../TODO-LIST.md)

---

## Du beta.2 au 1.0 — phase de stabilisation

beta.2 → 1.0 n'est pas une explosion de features. C'est le passage
d'un outil **qu'on raffine** à un outil **qu'on contractualise**.

À 1.0, l'API publique (MCP tool signatures, LSP custom methods, IR
types exportés par `standardoc-ir`, schema SQLite) est **gelée**. Ça
veut dire :

- Tout breaking change ultérieur passe par un bump `protocol_version`
  et une période de coexistence
- Les tools ne disparaissent pas silencieusement
- Le schema SQLite ne fait que migrer vers l'avant (jamais downgrade)
- Les benchmarks de scale sont **publiés** (cold start, watcher delta,
  MCP query latency p99 sur monorepos 1M+ LOC) — pas de "ça scale,
  faites-nous confiance"

C'est le contrat qui permet à des tiers de bâtir dessus en confiance.
Tant qu'on n'est pas à 1.0, on garde le droit de bouger la sémantique
d'un tool ; à 1.0 ce droit s'éteint sans accord explicite avec les
utilisateurs.

→ [Roadmap 1.0 détaillée dans TODO-LIST](../TODO-LIST.md)

---

## Les invariants qu'on protège dans cette phase

Quoi qu'il arrive entre maintenant et 1.0, ces invariants ne bougent
pas :

- **L'IR reste stable.** Les types dans `standardoc-ir` (`RawSymbol`,
  `RawEdge`, `EdgeKind`, `ResolvedOrUnresolved`, …) sont la grammaire
  cross-langage et cross-décennie du projet. On en ajoute si besoin,
  on n'en retire pas, et on ne change pas la sémantique d'un type
  existant sans bump `protocol_version`.
- **Le graphe reste local.** Pas de cloud sync, pas d'auth, pas de
  telemetry phone-home. Tout vit dans `.standardoc/` (gitignored,
  reproductible).
- **Le license timer reste armé.** Toute release garde la conversion
  automatique FSL-1.1-MIT → MIT pur 2 ans après sa date. La première
  release (`v1.0.0-beta.1`) convertit le 26 avril 2028. Aucun
  changement de termes rétroactif n'est possible — c'est l'engagement
  qu'on prend.
- **Le format SQLite reste ouvert.** Schema versionné, dump-able avec
  un sqlite3 standard, lisible sans aucun outil propriétaire. Si on
  disparaît demain, ton index continue de marcher.

---

## Pour aller plus loin

- [Vision moyen terme →](vision-moyen-terme.md) — beta.3 (doc rendering
  layer, CLI self-managed) et 1.x (post-stabilisation)
- [Vision long terme →](vision-long-terme.md) — UST + Lua plugin layer,
  écosystème, plateforme
- [Remarques →](remarques.md) — observations dogfood, décisions lockées
- [Retours de tests →](retours-tests.md) — ce qu'on a testé, ce qu'on a
  abandonné, les mesures
- [TODO-LIST →](../TODO-LIST.md) — checkboxes exhaustives par milestone

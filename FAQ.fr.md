# FAQ

[English](FAQ.md) · 📖 Français

---

## C'est un outil de documentation ?

Pas au sens TypeDoc / JSDoc. Standardoc est un **indexeur sémantique** — la
documentation narrative reviendra post-beta.1 comme output dérivé de l'index.
Day-1 le livrable c'est le graphe live + la surface MCP / LSP.

## Ça remplace LSP ?

Non, ça **complète** LSP. Le daemon LSP de Standardoc sert le graphe géré par
Standardoc ; rust-analyzer / tsserver continuent de servir leur surface
spécifique au langage. Utilisez les deux.

## Pourquoi seulement Rust + TypeScript en beta.1 ?

Deux langages bien supportés vaut mieux que dix langages à moitié supportés.
On perfectionne la base (unification FQDN, bridges cross-language, résolution
d'arêtes, perf sur monorepos 200k LOC) avant d'élargir.

## Quand le langage X sera-t-il supporté ?

Post-beta.1, dans cet ordre approximatif : Python, Go, Java, C#, Swift. Soit
via parsers natifs soit via tree-sitter. Ajouter un langage = implémenter le
trait `LanguageProvider` — voir [`crates/standardoc-lang-provider/`](crates/standardoc-lang-provider/).

## Comment installer ?

```sh
cargo install standardoc-cli
```

Ça vous donne le binaire `standardoc`. Pour le flow VSCode intégré, installez en
plus l'extension Standardoc. Walkthrough complet : [QUICKSTART.fr.md](QUICKSTART.fr.md).

## L'extension VSCode est obligatoire ?

Non. Le CLI marche standalone — vous pouvez lancer `standardoc lsp <ws>` + `standardoc
mcp <ws> --readonly` dans deux terminaux et utiliser Standardoc depuis Claude
Desktop, Cursor, le CLI Claude Code, ou n'importe quel client MCP-aware.
L'extension rend juste le flow seamless dans VSCode.

## Quelle différence vs `Read` / `Grep` / `Glob` natifs de Claude Code ?

Les natifs de Claude Code répondent aux questions **niveau texte**.
Standardoc répond aux questions **niveau graphe** — callers, callees,
imports, relations de types, arêtes cross-language — sans que l'agent ait à
assembler ces faits depuis des scans texte. ~100 tokens vs ~30k tokens par
question.

Le skill agent IA généré instruit Claude Code d'utiliser Standardoc **en
priorité** sur n'importe quelle tâche code, en fallback sur `Read` / `Grep` /
`Glob` seulement quand Standardoc ne peut pas répondre.

## Mon code est envoyé quelque part ?

Non. **Standardoc est local-only.** L'index vit dans `.standardoc/index.db`
sur votre disque. Le daemon MCP sert les données via `stdio` sur votre
machine. Pas d'appel réseau, pas de télémétrie, pas de SaaS.

## Comment ça performe sur de gros workspaces ?

Cold start sur le repo Standardoc lui-même (~600 fichiers, mixed Rust + TS)
prend moins d'une seconde. Overhead du watcher pendant les édits :
négligeable. SQLite + FTS5 scale bien jusqu'à des monorepos de 200k LOC. La
perf sur 1M+ LOC est sur l'agenda benchmark post-beta.1.

## Pourquoi open-core ?

Je suis le seul mainteneur, je travaille sur Standardoc en plus d'un job à
plein temps. La posture open-core permet à la toolchain dev de rester
gratuite (max portée d'écosystème) tout en gardant la porte ouverte à une
tier tooling payante optionnelle plus tard si le projet décolle. Quoi qu'il
arrive : pas de SaaS, pas d'abonnement, pas de télémétrie. Tant que
Standardoc n'a pas de composant cloud/serveur qui demande une infrastructure
récurrente, il n'y a aucune raison de facturer un abonnement — et aucun n'est
prévu. Tout ce qui serait payant serait local-only (tourne sur votre machine,
pas d'hébergement) et **licence à vie achat unique** — et seulement s'il y
a une vraie demande.

Le Core lui-même est verrouillé pour passer de FSL-1.1-MIT à **MIT pur le
26 avril 2028** quoi qu'il arrive.

## Pourquoi FSL-1.1-MIT et pas MIT pur ?

[FSL-1.1-MIT](LICENSE) est permissive pour tout **usage non-concurrent**.
Elle empêche les offerings concurrents directs (le pattern « open-and-pillage »)
sans verrouiller le core pour les end-users honnêtes. Adoptée par Sentry,
CodeCrafters, Keygen. Deux ans après chaque release, cette release convertit
en MIT pur.

## Je peux utiliser Standardoc commercialement ?

Oui, librement, tant que vous ne construisez pas un produit qui **se
substitue à Standardoc lui-même**. Tooling interne, apps customer-facing,
produits SaaS — tout ça OK. Revendre Standardoc comme votre propre SaaS —
pas OK. Voir la [licence](LICENSE) pour les détails.

## Ça perd en précision par rapport à LSP ?

Oui, intentionnellement — pour des raisons cross-language et de requête
AI-friendly. LSP donne une résolution parfaite par langage ; Standardoc donne
le graphe cross-language au prix d'un peu de profondeur per-language.
Utilisez les deux : LSP pour la précision éditeur, Standardoc pour le graphe
transverse + les requêtes IA.

## Et le rendu de doc / le DSL du prototype v0 ?

Le DSL de templating v0 (expressions `{{ @doc.X }}` dans le markdown) a été
**abandonné**. Il rendait le markdown illisible pour les auteurs humains et
difficile à maintenir sans UI dédiée.

Le remplacement, cible **beta.2**, est un package npm exposant des composants
React/MDX alimentés par le doc graph :

```mdx
<Doc id="user.create" />
<Params id="user.create" />
<Examples id="user.create" />

{queryDocs("api.*").map(d => <Doc key={d.id} id={d.id} />)}
```

Drop-in pour Next / Nextra / Astro / Docusaurus / n'importe quel framework
qui consomme du MDX. Le pipeline devient :

```
code source
  ↓
annotation parser (@doc)
  ↓
doc graph (SQLite)
  ↓
couche de rendering MDX / React (package npm)
  ↓
votre framework (Next / Nextra / Astro / Docusaurus / …)
```

Jusqu'à beta.2, l'index Standardoc se consomme exclusivement via MCP / LSP.

## Comment reporter un bug ou demander une feature ?

[GitHub Issues](https://github.com/miralabs-tech/standardoc/issues). Pour
les issues sécurité, email le mainteneur (voir `Cargo.toml` `authors`) — ne
postez pas publiquement tant qu'une politique `SECURITY.md` n'a pas ship.

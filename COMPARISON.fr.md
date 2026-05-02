# Comparaison

[English](COMPARISON.md) · 📖 Français

Comment Standardoc se positionne par rapport aux outils adjacents que vous
utilisez peut-être déjà.

---

## Vue d'ensemble

| Capacité                          | Standardoc | LSP (par-langage) | Grep / Sourcegraph | TypeDoc / JSDoc / Sphinx |
| --------------------------------- | :--------: | :---------------: | :----------------: | :----------------------: |
| Graphe cross-language             |     ✅     |        ❌         |         ⚠️         |            ❌            |
| Graphe sémantique (arêtes typées) |     ✅     |        ⚠️         |         ❌         |            ❌            |
| Surface MCP IA-first              |     ✅     |        ❌         |         ⚠️         |            ❌            |
| Index live (file watcher)         |     ✅     |        ✅         |         ❌         |            ❌            |
| Source de vérité = AST            |     ✅     |        ✅         |         ❌         |       ⚠️ (manuel)        |
| Aucune annotation requise         |     ✅     |        ✅         |         ✅         |            ❌            |
| Local-first, pas de SaaS          |     ✅     |        ✅         |       ⚠️ (B)       |            ✅            |
| Charge de setup                   |   Faible   |     Builtin       |    Faible (grep)   |       Moyenne-élevée     |

Légende : ✅ first-class · ⚠️ partiel / ça dépend · ❌ absent

---

## vs LSP (rust-analyzer, tsserver, …)

LSP est **complémentaire**, pas concurrent.

- LSP donne une résolution de symboles précise par langage, hover, go-to-
  definition, find-references, rename. Le daemon LSP de Standardoc fait la
  même chose pour la surface gérée par Standardoc.
- LSP est **par langage et par éditeur**. Standardoc unifie Rust + TS dans
  un graphe cross-language unique, requêtable via MCP depuis n'importe quel
  client IA.
- Le MCP de Standardoc expose le même graphe que le LSP sert à votre éditeur
  — même source de vérité, deux protocoles.

**Utilisez les deux.** LSP pour la navigation éditeur, Standardoc pour le
graphe cross-language + les requêtes des agents IA.

---

## vs Grep / Sourcegraph

Grep trouve du **texte**. Standardoc trouve du **sens**.

| Vous demandez                                    | Grep                                | Standardoc                                  |
| ------------------------------------------------ | ----------------------------------- | ------------------------------------------- |
| « Trouve tous les appels à `parse_workspace` »   | Toutes les occurrences en contexte  | Juste les vraies arêtes `CALLS`             |
| « De quoi dépend `createUser` ? »                | Walk manuel des fichiers            | `get_context(fqdn, depth=1)` → callees      |
| « Qui importe le module `Auth` ? »               | `grep -r "from .*Auth"`             | Liste d'arêtes `imported_by`, FQDN-résolues |
| « Quelle est la signature de cette fonction ? »  | Ouvrir le fichier, scroller         | `RawSymbol.signature` depuis `find_symbol`  |

Sourcegraph ajoute la recherche à l'échelle web et quelques features
sémantiques mais reste **text-centric** et **server-hosted**. Standardoc est
**graph-centric** et **local-only**.

---

## vs Outils de documentation (TypeDoc, JSDoc, Sphinx, Docusaurus)

Ces outils répondent à **« comment écrire de la prose narrative pour mon code »**.
Standardoc répond à **« comment exposer la structure de mon code aux agents
IA et au tooling »**.

- TypeDoc / JSDoc / Sphinx exigent des **annotations partout**. Standardoc
  marche sur **n'importe quelle codebase, sans annotations** — l'AST suffit.
- Les outils de doc produisent un **rendu statique** qui dérive dès que le
  code change. Standardoc garde l'index **live** via un file watcher.
- Les outils de doc ciblent les **lecteurs humains** de sites web. Standardoc
  cible les **agents IA, le tooling IDE, et les humains** via un seul
  contrat stable.

Une couche de rendering documentation est sur la roadmap post-beta.2 sous
forme de package npm exposant des composants React/MDX (`<Doc id="…" />`,
`<Params id="…" />`, `queryDocs("api.*")`) consommables depuis
Next/Nextra/Astro/Docusaurus/… Le doc graph (SQLite) alimente la couche
de rendering ; pas de moteur de template, pas de DSL custom — juste du
MDX avec des queries structurées. Une fois ship, vous aurez les deux :
structure live et prose narrative qui ne peut jamais dériver.

---

## vs Serveurs MCP par produit (Stripe MCP, GitHub MCP, …)

Ces serveurs exposent **un produit** aux agents IA. Standardoc expose
**votre codebase** aux agents IA — agnostique du produit, du framework, ou
du target de déploiement.

Vous utiliseriez un Stripe MCP pour gérer votre compte Stripe depuis un
agent. Vous utiliseriez Standardoc MCP pour que l'agent comprenne le code
que vous avez écrit qui *utilise* Stripe.

Complémentaires. Composez-les.

---

## Quand *ne pas* utiliser Standardoc

Réponse honnête :

- **Recherche de texte pure dans des fichiers** → utilisez Grep.
- **Patterns de chemins / glob** → utilisez Glob.
- **Lire un fichier connu à un chemin connu** → `cat` / open de votre éditeur.
- **Fichiers markdown / config sans rapport avec des symboles code** → hors scope.
- **Langages autres que Rust / TypeScript / JavaScript** → attendez post-beta.1,
  ou contribuez un `LanguageProvider` (voir
  [`crates/standardoc-lang-provider/`](crates/standardoc-lang-provider/)).

Standardoc est purpose-built pour la **compréhension sémantique de la
structure du code**. Hors de cette surface, les outils dédiés gagnent.

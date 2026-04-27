# À propos de Standardoc

[English](ABOUT.md) · 📖 Français

> **Pitch en une ligne** : un outil de doc qui résout **un seul problème** — ta doc ne dérive jamais de ton code, peu importe ce que tu refactores. Et au passage, expose ton codebase à n'importe quel agent IA via MCP, quel que soit le langage, même si le projet n'a jamais été init avec standardoc.

---

## Le problème

La documentation dérive. Toujours.

Tous les outils de doc d'aujourd'hui te demandent d'écrire la même information deux fois :
- une fois dans le code (signatures, types, contraintes, sémantique)
- une fois en markdown / JSDoc / fichier `.rst` / wiki / Notion / Confluence

À l'instant où le code change, la doc est fausse. Tu peux renommer un paramètre dans le code en une seconde. Mettre à jour les 47 fichiers markdown qui le mentionnent prend des heures — et personne ne le fait de façon cohérente. Six mois plus tard, la moitié de la doc ment à tes utilisateurs.

JSDoc, TypeDoc, Sphinx, Docusaurus en mode manuel, Nextra, GitBook — tous partagent le même défaut : le **lien entre le code et la prose est maintenu humainement**, et les humains cassent ce lien à chaque fois qu'ils shippent une feature sous deadline.

Pour les agents IA, le problème se multiplie. Pour répondre à "qu'est-ce que la fonction X prend en input ?", un agent aujourd'hui fait `grep -r "fn X" .` puis `cat src/foo.rs` puis devine. Coût : 30k–100k tokens par question. Avec un index doc fresh et structuré, la même réponse coûte ~100 tokens. **Une réduction 100x à 1000x**, systématiquement.

## La thèse

Découpler les **données structurées** (annotations à côté des symboles, machine-readable) de la **prose narrative** (markdown, écrite par un humain), et lier les deux avec un petit DSL que l'IDE comprend.

```rust
/// Additionne deux entiers.
/// @doc math.add add
/// @param a i32 premier opérande
/// @param b i32 deuxième opérande
/// @returns i32 la somme
pub fn add(a: i32, b: i32) -> i32 { a + b }
```

```markdown
## `{{ @doc.math.add:label }}`

{{ @doc.math.add:description }}

`{{ @doc.math.add:symbol.signature }}`

{{ each p in @doc.math.add:param }}
- **{{ p.name }}** (`{{ p.type }}`) : {{ p.description }}
{{ /each }}
```

Renommer `add` en `sum` dans le source ? La doc suit automatiquement. Changer le type d'un paramètre ? La doc suit. Supprimer un paramètre ? Le LSP gueule en rouge dans ton éditeur avec le diagnostic STD008.

L'annotation vit **à côté** de son symbole. La prose vit dans le markdown **séparément**. Le DSL fait le lien au moment du render. Déplace l'un ou l'autre, le lien survit. La dérive devient architecturalement impossible.

## Le double moat

### 1. Zéro drift entre code et prose

C'est le moat *fonctionnel*. TypeDoc / Nextra / Docusaurus en usage manuel forcent tous le dev à re-éditer la prose à chaque fois que le code bouge. Standardoc élimine cette étape. Une fois que t'as utilisé un outil qui garantit que la doc matche le code, revenir en arrière donne l'impression d'être imprudent.

### 2. Serveur MCP universel et language-agnostic

C'est le moat *stratégique*. Les serveurs MCP existants sont quasi tous :
- **product-specific** (Stripe MCP, Linear MCP, GitHub MCP, …) — utiles pour ce produit, inutiles pour ton code
- **library-specific** (un MCP par framework) — fragmentation, charge de maintenance

Standardoc est le premier serveur MCP qui :

- indexe **n'importe quel codebase** (Rust, TypeScript, Python en natif + tree-sitter pour Lua / et providers chargés dynamiquement)
- marche **même sur un projet jamais initialisé avec standardoc** : l'AST auto-découvre tous les exports, les annotations `@doc` ne sont qu'un enrichissement
- expose un seul protocole stable que les agents apprennent une fois et réutilisent partout — même set de tools que tu indexes un script de 200 lignes ou un monorepo de 200k LOC

Pour un agent IA, `{{ @doc.foo:description }}` résolu via MCP coûte ~100 tokens vs 10k–100k tokens de `grep + read` sur le repo. **100x à 1000x d'économie de tokens**, sur chaque question, chaque conversation, chaque projet.

## À qui ça s'adresse

Trois personas, trois modes d'usage :

### Le développeur (écrit les annotations)

Tu annotes les fonctions avec `@doc key`, optionnellement `@param name type description`, `@returns type description`, etc. Ton IDE (via le LSP) te donne la complétion, le hover, le goto-definition pour chaque référence, et le rename refactoring qui propage les changements de `DocKey` dans tous les fichiers `.md` automatiquement.

Tu passes 30 secondes par fonction à annoter, tu économises des semaines de maintenance de doc.

### Le lecteur de doc (consomme la sortie rendue)

Un utilisateur ouvre ton site de doc publié et lit exactement ce qui était dans le source au moment du build. Les signatures sont exactes. Les types des paramètres matchent. Les liens marchent. Les exemples sont du vrai code exécutable, pas des snippets fictionnalisés. La confiance restaurée.

### L'agent IA (interroge l'index)

Un agent (Claude Code, Cursor, Zed, Continue, …) se connecte via MCP, obtient 28 tools pour interroger l'index : list docs, search by type, find usages, validate doc syntax, génération d'exports llms.txt / skill.md / OpenAPI. L'agent n'a jamais à grep. L'agent n'hallucine jamais une signature. L'agent répond aux questions en 100 tokens, pas 100k.

## Open-core

Standardoc est **open-core, style GitLab**.

- **Standardoc Core** — CLI, LSP, MCP, tous les language providers, DSL, validator, plugins API, backend HTTP/SSE. Source sous [FSL-1.1-MIT](LICENSE) (passe en MIT pure après 2 ans). Libre pour tout usage non-concurrent.
- **Standardoc Pro** — l'UI web polish (navigation GitBook-like, composants MDX live, search, polish). Closed-source, achat **lifetime** unique, pas d'abonnement. Distribué comme un binaire.

Le pari : le dev tooling reste gratuit pour maximiser la portée écosystème (le moat c'est l'adoption). L'UI polish pour la création de doc non-dev est monétisée en achat unique pour financer mon travail sur le Core (parce qu'aucun coût d'infrastructure n'est soutenable sans revenu).

**Pro ne sort pas avant `v1.0.0`.** Tant que Standardoc est en `v0.x.x`, tout ce que je publie est OSS — le tier Pro est gardé en réserve pour que la surface d'API se stabilise d'abord, et que les utilisateurs payants reçoivent quelque chose qui ne casse pas la semaine d'après.

Pas de SaaS, pas d'abonnement par seat, pas de télémétrie, pas de modal d'upsell dans ton IDE. Paie une fois pour Pro si tu veux l'UI, sinon utilise le Core gratuitement à vie.

## Direction long terme

Ce qui arrive :

- **Extension VSCode** — thin wrapper qui spawn auto le daemon, surface le statut, ship indépendamment de `standardoc-server` lui-même
- **Chargement de grammaires WASM au runtime** — dépose un `tree-sitter-X.wasm` pour ajouter le support de n'importe quel langage avec une grammaire publique
- **Plus de language providers** — Java / Kotlin / Go / C# / Swift / Zig / etc. via tree-sitter une fois que le chargement WASM est livré
- **Résolution cross-ref FQN** — résolution propre des `use` / `import` par provider, met fin à l'ambiguïté short-name pour les gros workspaces
- **Features UI Pro** — snapshots de versions, analytics d'usage doc, génération d'annotations IA-assistée, team review queues, références cross-repo

Le backlog complet et le découpage en milestones vit dans les notes internes du projet — la roadmap publique est publiée en v0.1.1.

## Pourquoi ce projet existe

J'ai passé des années à écrire les mêmes fichiers markdown trois fois — une fois pour l'équipe, une fois pour le site web, une fois pour les outils IA qui n'ont pas lu le site web. Chaque réécriture fausse au moment où elle est finie. Chaque agent IA brûlant des milliers de tokens pour comprendre ce qu'une fonction de 5 lignes fait.

Avant les LLMs, la doc c'était 100% manuel. Quand t'es le seul à maintenir un projet open source en parallèle d'un boulot, écrire **et tenir à jour** la doc devient le bottleneck qui te fait abandonner. J'ai une pile de side projects que j'ai soit abandonnés, soit gardés privés à cause de ça — pas parce que la doc me dégoûte, mais parce que la garder synchro avec un codebase qui bouge prenait plus de temps qu'écrire le code lui-même. Standardoc c'est exactement le tooling que j'aurais voulu avoir pour ces projets.

Il devait y avoir une seule source de vérité que les humains pouvaient lire narrativement et que les agents pouvaient interroger structurellement — sans que je la réécrive pour chaque consommateur. C'est standardoc.

Si ça te fait gagner du temps, [soutiens le projet](README.fr.md#soutenir-le-projet).

Si tu trouves un bug ou veux une feature, [ouvre une issue](https://github.com/miralabs-tech/standardoc/issues).

Une fois `v1.0.0` shippée et [Standardoc Pro](/) livré, tu pourras prendre une licence lifetime unique pour l'UI web polish. D'ici là, tout est OSS et gratuit.

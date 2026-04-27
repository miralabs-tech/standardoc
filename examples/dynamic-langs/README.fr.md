# Providers de langage dynamiques

[English](README.md) · 📖 Français

Deux façons d'ajouter un langage à standardoc **sans recompiler** le binaire :

## 1. Fork tree-sitter (post-MVP — voir limites ci-dessous)

Réutilise une grammaire tree-sitter built-in (actuellement `lua`) avec des
patterns de query supplémentaires pour capturer des shapes de symboles
additionnelles. Fork = **ajouter des patterns**, pas **changer la grammaire**.

### Ce qu'il peut faire
- Ajouter de nouvelles captures sur la grammaire existante (ex : capturer
  `bind("name", function() end)` en plus des fns top-level)
- Surcharger les styles de commentaire (`---` vs `--`)
- Mapper vers un nouvel id / une nouvelle extension

### Ce qu'il NE PEUT PAS faire
- Ajouter de nouveaux opérateurs (`+=`, `-=`, `??=`, …) — la grammaire
  tree-sitter sous-jacente échouera à les parser
- Changer les keywords réservés ou les règles de tokens
- Ajouter des constructions syntaxiques (backtick hash strings comme le
  `joaat` de CfxLua, decorators, …)

Pour les vrais dialectes de langage avec changements de grammaire (CfxLua,
MoonScript, Teal, Fennel, …), la vraie solution est **une grammaire
tree-sitter dédiée compilée dans standardoc**, pas une config JSON. Le
chargement de grammaire au runtime (WASM) est sur la roadmap.

## 2. Provider regex (ce dossier : `exotic.json`)

Scan regex pure — pas d'AST, pas de dépendance grammaire. Marche sur
**n'importe quel** format texte. Moins précis (un keyword `function` dans
un literal string sera capturé aussi) mais couvre les langages sans
grammaire tree-sitter.

### Quand l'utiliser
- Langages niche / propriétaires sans grammaire tree-sitter publique
- Formats texte plats avec une structure function-like (DSL de config,
  fichiers de schéma, …)
- Prototypage rapide en attendant une vraie grammaire

### Référence du schéma

```json
{
  "id": "myx",
  "extensions": [".myx"],
  "commentStyles": {
    "single": ["#"],
    "docSingle": ["##"],
    "multi": { "start": "/*", "end": "*/" }
  },
  "backend": {
    "kind": "regex",
    "patterns": [
      { "kind": "function", "regex": "^\\s*fn\\s+(?P<name>\\w+)\\((?P<params>[^)]*)\\)" }
    ]
  }
}
```

Exigences sur les patterns :
- La capture `name` est **obligatoire**
- La capture `params` optionnelle, split par virgule en `ParamInfo`
- La capture `signature` optionnelle, utilisée comme override de la signature affichée
- Le champ `kind` mappe vers `SymbolKind` (`function`, `method`, `class`,
  `struct`, `enum`, `trait`, `module`, `field`, `variant`, `const`, …)

## Chargement

Dépose tes fichiers JSON dans `.standardoc/languages/` à la racine du
workspace. Standardoc les load au boot. Redémarre le daemon (lance
`./scripts/build.sh` ou `./scripts/build.ps1`, choisis `[2] prod`, puis
démarre une nouvelle conversation Claude Code) pour pick up les changements.

Les configs invalides sont logguées sur stderr et skippées — elles ne
bloquent pas les autres providers.

## Résolution de conflits

Si un provider dynamique déclare une extension déjà gérée par un provider
built-in (ex : `.lua`), le **built-in gagne** (enregistré en premier). Le
remplacement complet de provider est sur la roadmap.

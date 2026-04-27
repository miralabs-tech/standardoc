# users-api — démo "ratio effort/résultat"

[English](README.md) · 📖 Français

Mini-projet réaliste : une API REST `users` avec **5 endpoints**, côté serveur **Rust** + côté client **TypeScript**.

L'objectif de cette démo c'est de te montrer **deux approches** pour documenter les 10 fonctions, et de te faire voir que la version "smart" tient en **~57 lignes de template** pour produire la même page que la version "verbeuse" qui en fait ~170 — et la smart reste à 57 lignes peu importe combien d'endpoints tu ajoutes.

## Layout

```
users-api/
├── README.md                       ← tu es ici
├── COMPARISON.md                   ← regarde celui-là en premier
├── src/
│   ├── server.rs                   ← 5 endpoints Rust annotés `@doc users.X`
│   └── client.ts                   ← 5 wrappers TS annotés `@doc client.users.X`
├── docs-src/                       ← templates `.md` (sources, ce que TU écris)
│   ├── 01-api-verbose.md           ← template explicite : ~170 lignes
│   └── 02-api-smart.md             ← template smart : ~57 lignes
└── docs-rendered/                  ← markdown rendu (ce que voit l'utilisateur final)
    ├── 01-api-verbose.md           ← rendu de 01 (identique à 02)
    └── 02-api-smart.md             ← rendu de 02 (identique à 01)
```

## Pour voir tourner ça en vrai

Depuis la racine du repo `standardoc` :

```sh
cargo run -p standardoc -- transform examples/users-api examples/users-api/docs-src/02-api-smart.md
```

Tu obtiens en stdout exactement le contenu de `docs-rendered/02-api-smart.md`. Change un type dans `src/server.rs`, relance la commande, le rendu reflète le changement automatiquement. **Aucun `.md` à toucher.**

## Le punchline

- Ajouter une 6ème fonction dans `server.rs` ? Tu écris la fonction + son `@doc`. Tu touches **zéro template**. La page web/markdown se met à jour au prochain build.
- Avec la version verbeuse, faut éditer le template à la main pour ajouter une 6ème section. Et après pour la 7ème. Et pour la 8ème. C'est exactement le piège de TypeDoc / Docusaurus en usage manuel.

Ouvre `COMPARISON.md` pour voir les deux templates côte à côte.

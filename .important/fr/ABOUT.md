# À propos & carte de la documentation

[English](../en/ABOUT.md) · 📖 Français

Standardoc est une **infrastructure d'intelligence de code** : un IR
canonique multi-langues et un graphe sémantique vivant, dérivé de
l'AST, exposé à plusieurs surfaces (LSP, MCP, RAG). Un seul graphe
partagé — tes outils arrêtent de re-parser ton code chacun de leur
côté. Local, open-source, ~100 tokens par requête d'agent au lieu de
30k de grep + read.

→ Page d'accueil complète (pitch, posture, install, pour qui) :
**[README du hub](README.md)**.

Ce fichier-ci est la **carte** : où vit quoi, et dans quel ordre lire.

---

## Les documents

### Démarrer

- **[QUICKSTART.md](QUICKSTART.md)** — de zéro à un workspace indexé en
  ~5 minutes : extension VSCode, binaire, init, usage agent, CLI
  standalone, réglage de l'index.

### Comprendre le projet — `storytelling/`

Le narratif derrière les décisions. À lire si tu veux *pourquoi*, pas
seulement *comment*.

- **[philosophy.md](storytelling/philosophy.md)** — les 5 principes
  system-thinking, le diagnostic des approches existantes, l'éthique de
  construction, ce que Standardoc n'est PAS.
- **[vision-court-terme.md](storytelling/vision-court-terme.md)** —
  beta.2 (maturité) et la phase de stabilisation vers 1.0.
- **[vision-moyen-terme.md](storytelling/vision-moyen-terme.md)** —
  beta.3 (doc rendue, navigation visuelle, CLI autonome, compréhension
  cross-session) et le freeze 1.0 en profondeur.
- **[vision-long-terme.md](storytelling/vision-long-terme.md)** —
  l'inversion post-1.0 : le plug-in layer UST + Lua, le core qui ne
  grossit plus, l'écosystème.
- **[retours-tests.md](storytelling/retours-tests.md)** — observations
  dogfood : calibration agent, ce qu'on a abandonné, mesures honnêtes.
- **[remarques.md](storytelling/remarques.md)** — synthèse
  transversale : décisions structurantes, posture, réalité matérielle
  du projet.

### Décider si c'est pour toi

- **[FAQ.md](FAQ.md)** — questions courantes : positionnement,
  langages, licence, pricing, agents non-Claude, contribution.
- **[COMPARISON.md](COMPARISON.md)** — face à LSP, Sourcegraph,
  code-review-graph, Serena, Aider, Continue, SCIP/Glean/Kythe,
  générateurs de doc. Grille honnête + quand *ne pas* choisir
  Standardoc.

### Aller plus loin

- **[SUPPORT.md](SUPPORT.md)** — comment soutenir le projet,
  OpenCollective, opportunités de collaboration.
- **[SECURITY.md](SECURITY.md)** — politique de sécurité, distribution
  officielle, signalement de vulnérabilité.
- **[TODO-LIST.md](TODO-LIST.md)** — la roadmap exhaustive par
  milestone (`[x]` shippé · `[ ]` planifié · `~~barré~~` abandonné).

---

## Par où commencer

- **Tu évalues l'outil** → [README du hub](README.md) →
  [COMPARISON](COMPARISON.md) → [FAQ](FAQ.md)
- **Tu veux l'installer** → [QUICKSTART](QUICKSTART.md)
- **Tu veux comprendre la vision** →
  [philosophy](storytelling/philosophy.md) → les 3 visions
- **Tu veux suivre / contribuer** → [TODO-LIST](TODO-LIST.md) →
  [SUPPORT](SUPPORT.md)

---

> Version anglaise de toute cette documentation : [`.important/en/`](../en/).
> Code source et README racine : [`../../README.md`](../../README.md).

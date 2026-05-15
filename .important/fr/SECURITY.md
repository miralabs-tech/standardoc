# Politique de sécurité

[English](../../SECURITY.md) · 📖 Français

[Démarrage rapide](QUICKSTART.md) · [FAQ](FAQ.md) · [Support](SUPPORT.md)

## Versions supportées

Les mises à jour de sécurité ne sont actuellement fournies que pour la
dernière release beta supportée.

| Version | Supportée |
| ------- | --------- |
| 1.x beta | ✅ |
| < 1.0.0 | ❌ |

Les versions plus anciennes peuvent contenir des vulnérabilités
connues, des incompatibilités de protocole ou des dépendances
obsolètes, et ne sont plus maintenues.

---

## Distribution officielle

Les binaires officiels de Standardoc sont distribués uniquement via les
canaux officiels du projet, maintenus par miralabs-tech.

Sources officielles :

- https://github.com/miralabs-tech/standardoc
- https://github.com/miralabs-tech/standardoc/releases

Les distributions officielles peuvent inclure :

- des binaires CLI standalone
- des archives spécifiques par plateforme
- des binaires bundlés avec les extensions d'éditeur officielles
- des manifestes de version structurés
- des sommes de contrôle SHA256

Ne fais PAS confiance aux binaires, installeurs, miroirs, forks,
ré-uploads ou redistributions tierces se présentant comme Standardoc.

Les distributions non officielles peuvent être modifiées, obsolètes,
dangereuses ou malveillantes.

Standardoc ne distribue PAS officiellement :

- des bundles génériques de « logiciels »
- des installeurs sans rapport
- des binaires hébergés hors des canaux officiels du projet

Vérifie toujours :

- la source de la release
- les noms d'archives
- les sommes de contrôle SHA256
- la propriété du dépôt

avant d'exécuter un binaire téléchargé.

---

## Signaler une vulnérabilité

Si tu découvres une vulnérabilité de sécurité, signale-la en privé.

Tu peux :

- ouvrir un GitHub Security Advisory
- ou contacter les mainteneurs directement via GitHub

Inclus si possible :

- la version affectée
- le système d'exploitation
- les étapes de reproduction
- une description de l'impact
- une preuve de concept

Évite de divulguer publiquement une vulnérabilité avant qu'un correctif
soit disponible.

---

## Attentes en matière de sécurité

Standardoc est actuellement en développement beta actif.

Pendant la phase beta :

- les APIs peuvent évoluer
- les protocoles internes peuvent changer
- la compatibilité extension / runtime peut exiger des versions
  alignées

Il est recommandé de garder l'extension et les binaires runtime à jour.

Utilise toujours les distributions officielles et vérifie l'intégrité
des binaires avant exécution.

# Comparison — verbose vs smart

[English](COMPARISON.md) · 📖 Français

Tu as deux templates dans `docs-src/`. Les deux produisent **exactement le même rendu final** (regarde `docs-rendered/`). La différence c'est ce que TU écris et maintiens.

## Le verdict en chiffres

| | Lignes du template | Effort pour ajouter une 6ème fonction |
|---|---|---|
| `01-api-verbose.md` | **~170 lignes** | Éditer le template (copier-coller un bloc, renommer 8 fois la clé, espérer rien oublier) |
| `02-api-smart.md` | **~57 lignes** | Rien. La fonction apparaît automatiquement au prochain build. |

Et c'est avec **5 fonctions**. Avec 50, le verbeux explose à plus de 1500 lignes. Le smart fait toujours 57.

## La différence visuelle (le bout qui change)

### Verbeux — 1 fonction = 1 bloc à dupliquer

```markdown
### Create user

{{ @doc.users.create:description }}

​```rust
{{ @doc.users.create:symbol.signature }}
​```

{{ each p in @doc.users.create:param }}
- **{{ p.name }}** (`{{ p.type }}`): {{ p.description }}
{{ /each }}

**Returns** (`{{ @doc.users.create:returns.type }}`): {{ @doc.users.create:returns.description }}

### Get user by id

{{ @doc.users.get:description }}

​```rust
{{ @doc.users.get:symbol.signature }}
​```

{{ each p in @doc.users.get:param }}
- **{{ p.name }}** (`{{ p.type }}`): {{ p.description }}
{{ /each }}

**Returns** (`{{ @doc.users.get:returns.type }}`): {{ @doc.users.get:returns.description }}

### List users
... (et ainsi de suite pour chacune des 10 fonctions)
```

### Smart — 1 boucle pour les 5 fonctions du module

```markdown
{{ each f in @docs.module(users) }}
### {{ f.label }}

{{ f.description }}

​```rust
{{ f.symbol.signature }}
​```

{{ each p in f:param }}
- **{{ p.name }}** (`{{ p.type }}`): {{ p.description }}
{{ /each }}

**Returns** (`{{ f.returns.type }}`): {{ f.returns.description }}
{{ /each }}
```

C'est tout. Pour les 5 fonctions du serveur. **Et pour les 50 du jour où tu en auras 50.**

## Ce qui se passe quand tu changes le code

Imagine que tu décides de renommer le param `password` en `pwd` dans `server.rs` :

```diff
- /// @param password string mot de passe en clair, sera hashé en interne
+ /// @param pwd string mot de passe en clair, sera hashé en interne
- pub fn create(email: &str, password: &str) -> User { ... }
+ pub fn create(email: &str, pwd: &str) -> User { ... }
```

→ Au prochain build, **les deux templates** (verbeux ET smart) produisent une page où `password` est devenu `pwd` partout. **Zéro fichier `.md` à éditer.**

Compare avec une approche manuelle (Docusaurus, Nextra, FiveM-style) : tu aurais une page `.md` qui dit encore "password" jusqu'à ce qu'un humain pense à la mettre à jour. C'est exactement le scénario où la doc dérive du code et où, 6 mois plus tard, les gens ne font plus confiance à la doc.

## Et l'esthétique du template ?

Oui, écrit comme ça en Markdown brut sans coloration, le template ressemble à un mix `.md` + `mustache`. Mais :

1. **Tu l'écris une fois**, après tu y retournes presque jamais.
2. **L'extension VSCode** (LSP en place côté core, wrapper extension à finir) :
   - colore le DSL différemment du markdown
   - autocomplete sur `@doc.` → liste de tes blocks
   - hover sur un `{{ @doc.users.create:returns.type }}` → te montre `User` en live
   - goto-definition → t'envoie sur la fonction Rust correspondante
   - rename refactoring → renomme dans le code ET propage dans tous les `.md`
3. **Le résultat final** (`docs-rendered/`) c'est du markdown propre, lu par GitHub / VSCode preview / un site web — exactement ce que ton utilisateur final voit.

Tu n'écris pas le rendu, tu écris la "recette". La recette est moins jolie que le plat, c'est normal — mais tu l'écris UNE fois et tu manges 50 fois.

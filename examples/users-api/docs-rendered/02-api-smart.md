# Users API

API pour gérer les utilisateurs. Côté serveur (Rust) + côté client (TypeScript SDK).

> ✨ **Version "smart"** de ce template. Tu vois en bas — pas de `users.create`,
> `users.get`, `users.list` énumérés à la main. C'est `@docs.module(...)` qui
> itère sur tout ce qui matche. **Ajoute une 6ème fonction dans `server.rs`,
> elle apparaît automatiquement.**

## Server (Rust)

### Create user

Crée un nouvel utilisateur.

```rust
pub fn create(email: &str, password: &str) -> User
```

- **email** (`string`): adresse email (doit être unique)
- **password** (`string`): mot de passe en clair, sera hashé en interne

**Returns** (`User`): l'utilisateur fraîchement créé

_Since 1.0.0_

### Get user by id

Récupère un utilisateur par son identifiant.

```rust
pub fn get(id: u64) -> Option<User>
```

- **id** (`u64`): identifiant interne de l'utilisateur

**Returns** (`Option<User>`): Some(user) ou None si l'id n'existe pas

_Since 1.0.0_

### List users (paginated)

Liste paginée des utilisateurs.

```rust
pub fn list(page: u32, per_page: u32) -> Vec<User>
```

- **page** (`u32`): numéro de page (1-indexé)
- **per_page** (`u32`): taille de page, max 100

**Returns** (`Vec<User>`): les utilisateurs de la page demandée

_Since 1.0.0_

### Update user

Met à jour un utilisateur existant.

```rust
pub fn update(id: u64, patch: UserPatch) -> User
```

- **id** (`u64`): identifiant de l'utilisateur à modifier
- **patch** (`UserPatch`): champs à mettre à jour (tout est optionnel)

**Returns** (`User`): l'utilisateur après mise à jour

_Since 1.0.0_

### Delete user

Supprime un utilisateur. Soft delete — le record reste en base avec un flag.

```rust
pub fn delete(id: u64) -> bool
```

- **id** (`u64`): identifiant de l'utilisateur à supprimer

**Returns** (`bool`): true si supprimé, false si l'id n'existait pas

> ⚠️ **Deprecated**: préférer users.archive — sera retiré en 2.0

_Since 1.0.0_

## Client SDK (TypeScript)

### Create user (client)

Crée un utilisateur via l'API distante.

```ts
export async function createUser(email: string, password: string): Promise<User>
```

- **email** (`string`): adresse email du nouvel utilisateur
- **password** (`string`): mot de passe à transmettre au serveur

**Returns** (`Promise<User>`): l'utilisateur créé renvoyé par le serveur

### Get user (client)

Récupère un utilisateur par son id via l'API distante.

```ts
export async function getUser(id: number): Promise<User | null>
```

- **id** (`number`): identifiant de l'utilisateur

**Returns** (`Promise<User | null>`): l'utilisateur ou null si introuvable

### List users (client)

Liste paginée d'utilisateurs côté client.

```ts
export async function listUsers(page: number, perPage: number): Promise<User[]>
```

- **page** (`number`): numéro de page (1-indexé)
- **perPage** (`number`): taille de page, max 100

**Returns** (`Promise<User[]>`): les utilisateurs de la page

### Update user (client)

Met à jour un utilisateur côté client.

```ts
export async function updateUser(id: number, patch: Partial<User>): Promise<User>
```

- **id** (`number`): identifiant cible
- **patch** (`Partial<User>`): champs à modifier

**Returns** (`Promise<User>`): l'utilisateur mis à jour

### Delete user (client)

Supprime un utilisateur côté client.

```ts
export async function deleteUser(id: number): Promise<boolean>
```

- **id** (`number`): identifiant à supprimer

**Returns** (`Promise<boolean>`): true si supprimé

> ⚠️ **Deprecated**: préférer archiveUser — sera retiré en 2.0

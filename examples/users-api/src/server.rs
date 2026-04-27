//! HTTP handlers for the Users resource.
//! Each function is annotated once — docs update automatically
//! with every refactor.

/// Create a new user.
/// @doc users.create Create user
/// @param email string email address (must be unique)
/// @param password string plaintext password, hashed internally
/// @returns User newly created user
/// @since 1.0.0
pub fn create(email: &str, password: &str) -> User {
    todo!()
}

/// Fetch a user by identifier.
/// @doc users.get Get user by id
/// @param id u64 internal user identifier
/// @returns Option<User> Some(user) or None if id does not exist
/// @since 1.0.0
pub fn get(id: u64) -> Option<User> {
    todo!()
}

/// Paginated list of users.
/// @doc users.list List users (paginated)
/// @param page u32 page number (1-indexed)
/// @param per_page u32 page size, max 100
/// @returns Vec<User> users from requested page
/// @since 1.0.0
pub fn list(page: u32, per_page: u32) -> Vec<User> {
    todo!()
}

/// Update an existing user.
/// @doc users.update Update user
/// @param id u64 identifier of user to update
/// @param patch UserPatch fields to update (all optional)
/// @returns User user after update
/// @since 1.0.0
pub fn update(id: u64, patch: UserPatch) -> User {
    todo!()
}

/// Delete a user. Soft delete — record remains in DB with a flag.
/// @doc users.delete Delete user
/// @param id u64 identifier of user to delete
/// @returns bool true if deleted, false if id did not exist
/// @since 1.0.0
/// @deprecated prefer users.archive — will be removed in 2.0
pub fn delete(id: u64) -> bool {
    todo!()
}

pub struct User;
pub struct UserPatch;

/**
 * A user of the system.
 * @doc api.types.User User
 */
export interface User {
    id: number;
    name: string;
    email: string;
}

/**
 * Create a new user record.
 * @doc api.users.create Create user
 * @param name string user's display name
 * @param email string email used for login
 * @returns User the created user record
 */
export function createUser(name: string, email: string): User {
    return { id: 1, name, email };
}

/**
 * Delete a user by id.
 * @doc api.users.delete Delete user
 * @param id number identifier of the user to delete
 * @returns boolean whether a user was actually removed
 */
export function deleteUser(id: number): boolean {
    return id > 0;
}

/** Mapping type for event handlers that receive a user. */
export type UserHandler = (u: User) => Promise<void>;

// TypeScript client SDK for the users API.
// Each wrapper is annotated once — client docs stay aligned with TS
// signatures without manual intervention.

/**
 * Create a user through the remote API.
 * @doc client.users.create Create user (client)
 * @param email string email address of the new user
 * @param password string password to send to server
 * @returns Promise<User> created user returned by server
 * @since 1.0.0
 */
export async function createUser(email: string, password: string): Promise<User> {
  const res = await fetch("/api/users", { method: "POST", body: JSON.stringify({ email, password }) });
  return res.json();
}

/**
 * Fetch a user by id via the remote API.
 * @doc client.users.get Get user (client)
 * @param id number user identifier
 * @returns Promise<User | null> user or null if not found
 * @since 1.0.0
 */
export async function getUser(id: number): Promise<User | null> {
  const res = await fetch(`/api/users/${id}`);
  return res.status === 404 ? null : res.json();
}

/**
 * Client-side paginated list of users.
 * @doc client.users.list List users (client)
 * @param page number page number (1-indexed)
 * @param perPage number page size, max 100
 * @returns Promise<User[]> users for requested page
 * @since 1.0.0
 */
export async function listUsers(page: number, perPage: number): Promise<User[]> {
  const res = await fetch(`/api/users?page=${page}&per_page=${perPage}`);
  return res.json();
}

/**
 * Update a user on the client side.
 * @doc client.users.update Update user (client)
 * @param id number target identifier
 * @param patch Partial<User> fields to update
 * @returns Promise<User> updated user
 * @since 1.0.0
 */
export async function updateUser(id: number, patch: Partial<User>): Promise<User> {
  const res = await fetch(`/api/users/${id}`, { method: "PATCH", body: JSON.stringify(patch) });
  return res.json();
}

/**
 * Delete a user on the client side.
 * @doc client.users.delete Delete user (client)
 * @param id number identifier to delete
 * @returns Promise<boolean> true if deleted
 * @since 1.0.0
 * @deprecated prefer archiveUser — will be removed in 2.0
 */
export async function deleteUser(id: number): Promise<boolean> {
  const res = await fetch(`/api/users/${id}`, { method: "DELETE" });
  return res.ok;
}

export interface User {
  id: number;
  email: string;
}

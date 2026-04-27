/**
 * Calls the greet endpoint.
 * @doc client.greet Greet client
 * @param name string name to send to the server
 * @returns Promise<string> the message returned by the server
 * @since 0.1.0
 */
export async function greet(name: string): Promise<string> {
    const res = await fetch(`/greet?name=${name}`);
    return res.text();
}

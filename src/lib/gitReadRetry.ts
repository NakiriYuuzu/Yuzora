/** Only retry a read rejected before execution because its bounded worker is busy. */
export async function retryBusyGitRead<T>(read: () => Promise<T>, current: () => boolean): Promise<T> {
    for (let attempt = 0; ; attempt++) {
        try { return await read() }
        catch (error) {
            if (attempt >= 2 || !current() || !/^(?:Error: )?(git-jobs-busy|host-git-limit)$/.test(String(error))) throw error
            await new Promise(resolve => setTimeout(resolve, attempt === 0 ? 50 : 150))
            if (!current()) throw error
        }
    }
}

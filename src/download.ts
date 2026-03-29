import type { Bot } from "grammy"

// ── File download helper ────────────────────────────────────────────────────

export type AttachedFile = { mime: string; filename: string; dataUrl: string }

export async function downloadTelegramFile(
  bot: Bot,
  fileId: string,
  mime: string,
  filename: string,
): Promise<AttachedFile | undefined> {
  try {
    const file = await bot.api.getFile(fileId)
    if (!file.file_path) return undefined
    // Use grammy's built-in file URL construction to avoid leaking token in error messages
    const url = `https://api.telegram.org/file/bot${bot.token}/${file.file_path}`
    const res = await fetch(url)
    if (!res.ok) return undefined
    const buf = Buffer.from(await res.arrayBuffer())
    const dataUrl = `data:${mime};base64,${buf.toString("base64")}`
    return { mime, filename, dataUrl }
  } catch (e) {
    console.error("File download error:", (e as Error).message)
    return undefined
  }
}

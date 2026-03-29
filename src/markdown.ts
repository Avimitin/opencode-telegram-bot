// ── MarkdownV2 formatting ───────────────────────────────────────────────────

const MD2_SPECIAL = /([_*\[\]()~`>#+\-=|{}.!\\])/g

function escapeMd2(text: string): string {
  return text.replace(MD2_SPECIAL, "\\$1")
}

// Sentinel characters for placeholder substitution.
// Using Unicode private-use-area codepoints instead of raw control bytes
// to avoid collisions with any real content.
const PH = {
  IC_START: "\uE000", IC_END: "\uE001",
  LK_START: "\uE002", LK_END: "\uE003",
  BS: "\uE004", BE: "\uE005",
  IS: "\uE006", IE: "\uE007",
}

export function toMarkdownV2(text: string): string {
  const segments: string[] = []

  const codeBlockRe = /```(\w*)\n([\s\S]*?)```/g

  // Find all code blocks
  const blocks: { start: number; end: number; lang: string; code: string }[] = []
  let m: RegExpExecArray | null
  while ((m = codeBlockRe.exec(text)) !== null) {
    blocks.push({ start: m.index, end: m.index + m[0].length, lang: m[1]!, code: m[2]! })
  }

  // Build parts array
  const parts: { type: "text" | "codeblock"; content: string; lang?: string }[] = []
  let lastEnd = 0
  for (const block of blocks) {
    if (block.start > lastEnd) {
      parts.push({ type: "text", content: text.slice(lastEnd, block.start) })
    }
    parts.push({ type: "codeblock", content: block.code, lang: block.lang })
    lastEnd = block.end
  }
  if (lastEnd < text.length) {
    parts.push({ type: "text", content: text.slice(lastEnd) })
  }

  const inlineCodeRe = /`([^`\n]+)`/g

  for (const part of parts) {
    if (part.type === "codeblock") {
      const escaped = part.content.replace(/\\/g, "\\\\").replace(/`/g, "\\`")
      segments.push(`\`\`\`${part.lang ?? ""}\n${escaped}\`\`\``)
    } else {
      let t = part.content

      // Temporarily replace inline code
      const inlineCodes: string[] = []
      t = t.replace(inlineCodeRe, (_, code) => {
        const idx = inlineCodes.length
        const escaped = code.replace(/\\/g, "\\\\").replace(/`/g, "\\`")
        inlineCodes.push("`" + escaped + "`")
        return `${PH.IC_START}${idx}${PH.IC_END}`
      })

      // Temporarily replace links [text](url)
      const links: string[] = []
      t = t.replace(/\[([^\]]+)\]\(([^)]+)\)/g, (_, linkText, url) => {
        const idx = links.length
        links.push(`[${escapeMd2(linkText)}](${url.replace(/[)\\]/g, "\\$&")})`)
        return `${PH.LK_START}${idx}${PH.LK_END}`
      })

      // Convert **bold** → *bold* (Telegram uses single *)
      t = t.replace(/\*\*(.+?)\*\*/g, (_, inner) => `${PH.BS}${inner}${PH.BE}`)

      // Convert single *italic* → _italic_
      t = t.replace(/(?<!\*)\*(?!\*)(.+?)(?<!\*)\*(?!\*)/g, (_, inner) => `${PH.IS}${inner}${PH.IE}`)

      // Escape remaining special chars
      t = escapeMd2(t)

      // Restore placeholders
      t = t.replace(new RegExp(PH.BS, "g"), "*").replace(new RegExp(PH.BE, "g"), "*")
      t = t.replace(new RegExp(PH.IS, "g"), "_").replace(new RegExp(PH.IE, "g"), "_")

      const icRe = new RegExp(`${PH.IC_START}(\\d+)${PH.IC_END}`, "g")
      t = t.replace(icRe, (_, idx) => inlineCodes[Number(idx)]!)

      const lkRe = new RegExp(`${PH.LK_START}(\\d+)${PH.LK_END}`, "g")
      t = t.replace(lkRe, (_, idx) => links[Number(idx)]!)

      segments.push(t)
    }
  }

  return segments.join("")
}

export function thinkingToMd2(text: string): string {
  const escaped = escapeMd2(text)
  const lines = escaped.split("\n")
  if (lines.length === 0) return ""
  const result = lines.map((l, i) => {
    const prefix = i === 0 ? ">💭 " : ">"
    const suffix = i === lines.length - 1 ? "||" : ""
    return `${prefix}${l}${suffix}`
  })
  return result.join("\n")
}

export function splitMessage(text: string, limit: number): string[] {
  if (text.length <= limit) return [text]
  const chunks: string[] = []
  let remaining = text
  while (remaining.length > 0) {
    if (remaining.length <= limit) {
      chunks.push(remaining)
      break
    }
    let splitAt = remaining.lastIndexOf("\n", limit)
    if (splitAt < limit / 2) splitAt = limit
    chunks.push(remaining.slice(0, splitAt))
    remaining = remaining.slice(splitAt).trimStart()
  }
  return chunks
}

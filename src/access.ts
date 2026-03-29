import { readFileSync, writeFileSync, mkdirSync } from "fs"
import { randomBytes } from "crypto"
import { ACCESS_FILE, STATE_DIR } from "./config.js"

// ── Types ──────────────────────────────────────────────────────────────────

export type PendingEntry = {
  senderId: string
  chatId: string
  createdAt: number
  expiresAt: number
  replies: number
}

export type GroupPolicy = {
  requireMention: boolean
  allowFrom: string[]
}

export type Access = {
  dmPolicy: "pairing" | "allowlist" | "disabled"
  allowFrom: string[]
  groups: Record<string, GroupPolicy>
  pending: Record<string, PendingEntry>
  mentionPatterns?: string[]
}

// ── Cached access ──────────────────────────────────────────────────────────

let cachedAccess: Access | undefined
let cacheTime = 0
const CACHE_TTL_MS = 2000

export function loadAccess(): Access {
  const now = Date.now()
  if (cachedAccess && now - cacheTime < CACHE_TTL_MS) return cachedAccess
  try {
    cachedAccess = JSON.parse(readFileSync(ACCESS_FILE, "utf8"))
    cacheTime = now
    return cachedAccess!
  } catch {
    cachedAccess = { dmPolicy: "pairing", allowFrom: [], groups: {}, pending: {} }
    cacheTime = now
    return cachedAccess
  }
}

export function saveAccess(access: Access) {
  mkdirSync(STATE_DIR, { recursive: true })
  writeFileSync(ACCESS_FILE, JSON.stringify(access, null, 2))
  cachedAccess = access
  cacheTime = Date.now()
}

export function invalidateAccessCache() {
  cachedAccess = undefined
}

// ── Pairing ────────────────────────────────────────────────────────────────

function generatePairingCode(): string {
  return randomBytes(3).toString("hex")
}

export function handlePairing(senderId: string, chatId: string): string {
  const access = loadAccess()

  if (access.allowFrom.includes(senderId)) {
    return "You are already paired. Send messages and I will respond."
  }

  const existing = Object.entries(access.pending).find(([, v]) => v.senderId === senderId)
  if (existing) {
    const [code, entry] = existing
    if (entry.expiresAt > Date.now()) {
      entry.replies = (entry.replies || 0) + 1
      if (entry.replies > 3) {
        delete access.pending[code]
        saveAccess(access)
        return "Too many attempts. Please try again later."
      }
      saveAccess(access)
      return `Your pairing code is: ${code}\nAsk the admin to run: opencode telegram pair ${code}`
    }
    delete access.pending[code]
  }

  // Clean expired
  for (const [code, entry] of Object.entries(access.pending)) {
    if (entry.expiresAt < Date.now()) delete access.pending[code]
  }

  if (Object.keys(access.pending).length >= 3) {
    saveAccess(access)
    return "Too many pending pairing requests. Please try again later."
  }

  const code = generatePairingCode()
  access.pending[code] = {
    senderId,
    chatId,
    createdAt: Date.now(),
    expiresAt: Date.now() + 3600_000,
    replies: 1,
  }
  saveAccess(access)
  return `Your pairing code is: ${code}\nAsk the admin to approve with: opencode telegram pair ${code}\nThis code expires in 1 hour.`
}

// ── Gate ────────────────────────────────────────────────────────────────────

export function getMentionPatterns(botUsername: string): string[] {
  const access = loadAccess()
  const patterns = [...(access.mentionPatterns ?? [])]
  // Always include the bot's own @username
  if (botUsername) {
    const atUsername = `@${botUsername}`
    if (!patterns.includes(atUsername)) patterns.push(atUsername)
  }
  return patterns
}

export function gate(ctx: any, botInfoId: number | undefined, botUsername: string): "allow" | "pair" | "deny" {
  const access = loadAccess()
  const senderId = String(ctx.from?.id ?? "")
  const chatId = String(ctx.chat?.id ?? "")

  // Group message
  if (ctx.chat?.type === "group" || ctx.chat?.type === "supergroup") {
    const groupPolicy = access.groups[chatId]
    if (!groupPolicy) return "deny"

    if (groupPolicy.requireMention) {
      const text = ctx.message?.text ?? ctx.message?.caption ?? ""
      const patterns = getMentionPatterns(botUsername)
      const startsWithMention = patterns.some((p: string) => text.trimStart().startsWith(p))
      const replyToBot = ctx.message?.reply_to_message?.from?.id === botInfoId
      if (!startsWithMention && !replyToBot) return "deny"
    }

    if (groupPolicy.allowFrom.length > 0 && !groupPolicy.allowFrom.includes(senderId)) {
      return "deny"
    }
    return "allow"
  }

  // DM
  if (access.dmPolicy === "disabled") return "deny"
  if (access.allowFrom.includes(senderId)) return "allow"
  if (access.dmPolicy === "pairing") return "pair"
  return "deny"
}

// ── Mention stripping ──────────────────────────────────────────────────────

export function stripMention(text: string, botUsername: string): string {
  const patterns = getMentionPatterns(botUsername)
  for (const p of patterns) {
    if (text.trimStart().startsWith(p)) {
      return text.trimStart().slice(p.length).trimStart()
    }
  }
  return text
}

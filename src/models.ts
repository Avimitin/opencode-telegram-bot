// ── Model picker ────────────────────────────────────────────────────────────

export type ModelEntry = { fullId: string; label: string }

let cachedModels: ModelEntry[] | undefined

export async function getModelList(opencode: any): Promise<ModelEntry[]> {
  if (cachedModels) return cachedModels
  const result = await opencode.client.provider.list()
  if (result.error || !result.data) return []
  const models: ModelEntry[] = []
  for (const provider of result.data.all) {
    const envConfigured = provider.env.length === 0 || provider.env.some((e: string) => !!process.env[e])
    if (!envConfigured) continue
    for (const model of Object.values(provider.models) as any[]) {
      const fullId = `${provider.id}/${model.id}`
      const tags: string[] = []
      if (model.reasoning) tags.push("🧠")
      const modalities = model.modalities?.input ?? []
      if (modalities.includes("image")) tags.push("🖼")
      if (model.attachment) tags.push("📎")
      const tagStr = tags.length ? ` ${tags.join("")}` : ""
      models.push({ fullId, label: `${fullId}${tagStr}` })
    }
  }
  cachedModels = models
  setTimeout(() => { cachedModels = undefined }, 300_000)
  return models
}

const MODELS_PER_PAGE = 6

export function buildModelKeyboard(models: ModelEntry[], page: number) {
  const totalPages = Math.ceil(models.length / MODELS_PER_PAGE)
  const start = page * MODELS_PER_PAGE
  const pageModels = models.slice(start, start + MODELS_PER_PAGE)
  const rows: { text: string; callback_data: string }[][] = []

  if (totalPages > 1 && page > 0) {
    rows.push([{ text: `⬅️ Page ${page}`, callback_data: `modelpage:${page - 1}` }])
  }

  for (const m of pageModels) {
    rows.push([{ text: m.label, callback_data: `model:${m.fullId}` }])
  }

  if (totalPages > 1 && page < totalPages - 1) {
    rows.push([{ text: `Page ${page + 2} ➡️`, callback_data: `modelpage:${page + 1}` }])
  }

  return { inline_keyboard: rows }
}

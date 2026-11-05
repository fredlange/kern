// The whole backend is portal.k behind `kern host serve`.
// POST /NAME takes exactly the definition's parameters; GET /state is the world.
// Status codes come from the host: 200 ok, 409 a declared refusal, 400 bad call, 403 not proven.

const API = '/api'

async function call(name, args) {
  const r = await fetch(`${API}/${name}`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(args),
  })
  const body = await r.json().catch(() => ({}))
  if (r.status === 200) return { ok: true, ...body }
  if (r.status === 409) return { ok: false, refused: body.refused, args: body.args || [] }
  return { ok: false, error: body.error || `HTTP ${r.status}` }
}

export const SOURCES = { 1: 'joke', 2: 'pun' }

export const request = src => call('request', { src })
export const upvote = item => call('upvote', { item })
export const giveUp = src => call('give_up', { src })

export async function loadWorld() {
  const r = await fetch(`${API}/state`)
  if (!r.ok) throw new Error(`GET /state: HTTP ${r.status}`)
  const { state } = await r.json()
  const m = name => state[name] || {}
  const ref = m('ref'), source = m('source'), votes = m('votes'), pending = m('pending')
  const items = Object.keys(ref)
    .filter(k => ref[k] > 0)
    .map(k => ({
      slot: Number(k),
      source: SOURCES[source[k]] || '?',
      ref: ref[k],
      votes: votes[k] || 0,
      text: textOf(state, k),
    }))
  return { items, pending: { 1: pending[1] === 1, 2: pending[2] === 1 } }
}

// The joke itself: `state body Int -> Text`, filled by store (via store_joke / store_pun) from the reply.
function textOf(state, slot) {
  const text = (state.body || {})[slot]
  return typeof text === 'string' && text.length > 0 ? text : null
}

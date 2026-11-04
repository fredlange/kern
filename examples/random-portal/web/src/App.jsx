import React, { useCallback, useEffect, useState } from 'react'
import { giveUp, loadWorld, request, upvote } from './kern.js'

const REFUSALS = {
  AlreadyPending: 'Already fetching one of those. Kern allows one request per source at a time.',
  NotYet: 'Still within the 30-second grace period.',
  NoSuchItem: 'That item does not exist.',
  UnknownSource: 'Unknown source.',
}

function explain(r) {
  if (r.refused) return `Kern refused: ${r.refused}${r.args.length ? ' ' + r.args.join(' ') : ''}. ${REFUSALS[r.refused] || ''}`
  return `Error: ${r.error}`
}

export default function App() {
  const [world, setWorld] = useState({ items: [], pending: {} })
  const [order, setOrder] = useState('new')
  const [note, setNote] = useState(null)
  const [down, setDown] = useState(null)

  const refresh = useCallback(async () => {
    try {
      setWorld(await loadWorld())
      setDown(null)
    } catch (e) {
      setDown(String(e.message || e))
    }
  }, [])

  useEffect(() => {
    refresh()
    const t = setInterval(refresh, 2000)
    return () => clearInterval(t)
  }, [refresh])

  const act = async (p) => {
    const r = await p
    setNote(r.ok ? null : explain(r))
    refresh()
  }

  const items = [...world.items].sort((a, b) =>
    order === 'top' ? b.votes - a.votes || b.slot - a.slot : b.slot - a.slot)

  return (
    <div className="page">
      <header>
        <h1>The Random Information Portal</h1>
        <p className="tag">Useless, funny, and backed by proofs.</p>
      </header>

      <section className="controls">
        {[[1, 'Fetch a joke'], [2, 'Fetch a pun']].map(([src, label]) => (
          <span key={src} className="fetch">
            <button onClick={() => act(request(src))} disabled={world.pending[src]}>
              {world.pending[src] ? 'Fetching…' : label}
            </button>
            {world.pending[src] && (
              <button className="ghost" onClick={() => act(giveUp(src))}>give up</button>
            )}
          </span>
        ))}
        <span className="order">
          <button className={order === 'new' ? 'on' : 'ghost'} onClick={() => setOrder('new')}>Newest</button>
          <button className={order === 'top' ? 'on' : 'ghost'} onClick={() => setOrder('top')}>Funniest</button>
        </span>
      </section>

      {down && <p className="note bad">Backend unreachable: {down}</p>}
      {note && <p className="note">{note}</p>}

      <ul className="items">
        {items.length === 0 && !down && <li className="empty">Nothing yet. Fetch something silly.</li>}
        {items.map(it => (
          <li key={it.slot} className="item">
            <div className="meta">
              <span className={`badge ${it.source}`}>{it.source}</span>
              <span className="slot">#{it.slot}</span>
            </div>
            <p className="text">
              {it.text ?? <span className="missing">{it.source} #{it.ref} at the source. The text is not in Kern yet (no Text type).</span>}
            </p>
            <button className="vote" onClick={() => act(upvote(it.slot))} aria-label={`Upvote item ${it.slot}`}>
              ▲ {it.votes}
            </button>
          </li>
        ))}
      </ul>

      <footer>
        Every button is a Kern definition the host only runs because it is proven.{' '}
        <a href="/proof.html">Read the proof page</a> · <a href="/host.html">what the host assumes</a> · <a href="/api/manifest">permission manifest</a>
      </footer>
    </div>
  )
}

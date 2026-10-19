// Guesses which columns of a schedules / attributes table play which role,
// for the load form. The user can override every guess.

import type { Inspection, TableMapping } from './engine/types'

export interface SchedRoles {
  id: string | null
  act: string | null
  start: string | null
  end: string | null
  duration: string | null
  seq: string | null
}

const CANDIDATES: Record<keyof SchedRoles, string[]> = {
  id: ['pid', 'person_id', 'personid', 'person', 'agent_id', 'agent', 'individual_id', 'indiv_id', 'id', 'puid', 'uid'],
  act: ['act', 'activity', 'activity_type', 'act_type', 'purpose', 'label', 'type', 'episode', 'state'],
  start: ['start', 'start_time', 'starttime', 'start_min', 'start_minute', 'begin', 'from', 'tstart', 't0'],
  end: ['end', 'end_time', 'endtime', 'end_min', 'end_minute', 'stop', 'finish', 'to', 'tend', 't1'],
  duration: ['duration', 'dur', 'duration_min', 'minutes', 'length'],
  seq: ['seq', 'sequence', 'seq_no', 'order', 'episode_no', 'act_no', 'index'],
}

const norm = (s: string) => s.trim().toLowerCase().replace(/[\s-]+/g, '_')

/** The first column matching a candidate name (exact, case-insensitive), if any. */
function pick(columns: string[], names: string[], taken: Set<string>): string | null {
  const byNorm = new Map(columns.filter((c) => !taken.has(c)).map((c) => [norm(c), c]))
  for (const n of names) {
    const hit = byNorm.get(n)
    if (hit) return hit
  }
  return null
}

export function guessRoles(columns: string[]): SchedRoles {
  const taken = new Set<string>()
  const take = (k: keyof SchedRoles) => {
    const c = pick(columns, CANDIDATES[k], taken)
    if (c) taken.add(c)
    return c
  }
  const id = take('id')
  const act = take('act')
  const start = take('start')
  const end = take('end')
  const duration = end ? null : take('duration')
  const seq = take('seq')
  return { id, act, start, end, duration, seq }
}

/** Whether a table looks like a schedules table (has activity and start columns). */
export function looksLikeSchedules(columns: string[]): boolean {
  const r = guessRoles(columns)
  return r.act !== null && r.start !== null && (r.end !== null || r.duration !== null)
}

/** The attributes table's id column: same name as the schedules id, else a guess. */
export function guessAttrId(columns: string[], schedId: string | null): string | null {
  if (schedId && columns.includes(schedId)) return schedId
  return pick(columns, CANDIDATES.id, new Set()) ?? columns[0] ?? null
}

/** Problems that stop a load, as messages (empty when the mapping is complete). */
export function mappingProblems(m: Partial<TableMapping>, sched: Inspection | null): string[] {
  if (!sched) return ['Choose a schedules table.']
  const out: string[] = []
  if (!m.id_col) out.push('Choose the person id column.')
  if (!m.act_col) out.push('Choose the activity column.')
  if (!m.start_col) out.push('Choose the start column.')
  if (!m.end_col && !m.duration_col) out.push('Choose the end (or duration) column.')
  const axis = m.axis
  if (axis && !(axis[1] - axis[0] >= 1 && axis[1] - axis[0] <= 2048)) {
    out.push('The time axis must be 1 minute to 34 h 8 min long.')
  }
  return out
}

export function formatBytes(n: number): string {
  if (n < 1e3) return `${n} B`
  if (n < 1e6) return `${(n / 1e3).toFixed(0)} kB`
  if (n < 1e9) return `${(n / 1e6).toFixed(1)} MB`
  return `${(n / 1e9).toFixed(2)} GB`
}

/** File kinds the loader accepts. */
export function fileKind(name: string): 'overact' | 'table' | null {
  const n = name.toLowerCase()
  if (n.endsWith('.overact')) return 'overact'
  if (/\.(csv|tsv|tab|txt|parquet|pq)$/.test(n)) return 'table'
  return null
}

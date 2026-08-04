import type { AlbumFilter } from '../../shared/albumPresentation'

export interface FilterBarEvents { onChange: (filter: AlbumFilter) => void }
type Part = 'year' | 'month' | 'day'

export function renderFilterBar(filter: AlbumFilter, dates: string[]): string {
  const value = selectedParts(filter)
  return `<div class="fan-filter-bar" id="fan-filter-bar" aria-label="按时间筛选相册">
    <div class="fan-filter-controls">
      ${trigger('year', value.year ? `${value.year} 年` : '全部年份', false, Boolean(value.year))}
      ${trigger('month', value.month ? `${value.month} 月` : '全部月份', !value.year, Boolean(value.month))}
      ${trigger('day', value.day ? `${value.day} 日` : '全部日期', !value.month, Boolean(value.day))}
    </div>
    <div class="fan-filter-options" id="fan-filter-options" role="listbox" aria-label="时间选项" hidden></div>
  </div>`
}

export function mountFilterBar(container: HTMLElement, current: AlbumFilter, dates: string[], events: FilterBarEvents): () => void {
  container.innerHTML = renderFilterBar(current, dates)
  const controls = container.querySelector<HTMLElement>('.fan-filter-controls')!
  const options = container.querySelector<HTMLElement>('#fan-filter-options')!
  const selected = selectedParts(current)
  const open = (part: Part) => {
    const values = availableValues(part, selected, dates)
    const empty = part === 'year' ? '全部年份' : part === 'month' ? '全部月份' : '全部日期'
    options.innerHTML = `<button type="button" role="option" class="fan-filter-option fan-filter-clear ${selected[part] ? '' : 'is-active'}" data-value="" aria-selected="${!selected[part]}">${empty}</button>${values.map((value) => `<button type="button" role="option" class="fan-filter-option ${selected[part] === value ? 'is-active' : ''}" data-value="${value}" aria-selected="${selected[part] === value}">${part === 'year' ? `${value} 年` : part === 'month' ? `${value} 月` : `${value} 日`}</button>`).join('')}`
    options.hidden = false
    options.dataset.part = part
    controls.querySelectorAll<HTMLButtonElement>('[data-part]').forEach((button) => {
      button.setAttribute('aria-expanded', String(button.dataset.part === part))
    })
  }
  const close = () => {
    options.hidden = true
    delete options.dataset.part
    controls.querySelectorAll<HTMLButtonElement>('[data-part]').forEach((button) => button.setAttribute('aria-expanded', 'false'))
  }
  const onControl = (event: Event) => {
    const button = (event.target as Element).closest<HTMLButtonElement>('[data-part]')
    if (!button || button.disabled) return
    open(button.dataset.part as Part)
  }
  const onOption = (event: Event) => {
    const button = (event.target as Element).closest<HTMLButtonElement>('[data-value]')
    const part = options.dataset.part as Part | undefined
    if (!button || !part) return
    const next = { ...selected, [part]: button.dataset.value ?? '' }
    if (part === 'year') { next.month = ''; next.day = '' }
    if (part === 'month') next.day = ''
    close()
    events.onChange(yearMonthDayFilter(next.year, next.month, next.day))
  }
  const onDocumentClick = (event: MouseEvent) => {
    if (!options.hidden && !container.contains(event.target as Node)) close()
  }
  const onKeyDown = (event: KeyboardEvent) => {
    if (event.key === 'Escape' && !options.hidden) close()
  }
  controls.addEventListener('click', onControl)
  options.addEventListener('click', onOption)
  document.addEventListener('click', onDocumentClick)
  document.addEventListener('keydown', onKeyDown)
  return () => {
    controls.removeEventListener('click', onControl)
    options.removeEventListener('click', onOption)
    document.removeEventListener('click', onDocumentClick)
    document.removeEventListener('keydown', onKeyDown)
  }
}

export function yearMonthDayFilter(year: string, month: string, day: string): AlbumFilter {
  if (!/^\d{4}$/.test(year)) return {}
  if (!/^(0[1-9]|1[0-2])$/.test(month)) return { from: `${year}-01-01`, to: `${year}-12-31` }
  if (!/^(0[1-9]|[12]\d|3[01])$/.test(day)) return { from: `${year}-${month}-01`, to: `${year}-${month}-${String(daysInMonth(year, month)).padStart(2, '0')}` }
  return { from: `${year}-${month}-${day}`, to: `${year}-${month}-${day}` }
}

function trigger(part: Part, text: string, disabled: boolean, active: boolean): string {
  const label = part === 'year' ? '选择年份' : part === 'month' ? '选择月份' : '选择日期'
  return `<button class="fan-filter-trigger ${active ? 'is-active' : ''}" type="button" data-part="${part}" aria-label="${label}" aria-haspopup="listbox" aria-expanded="false"${disabled ? ' disabled' : ''}>${text}<span aria-hidden="true"></span></button>`
}
function selectedParts(filter: AlbumFilter): { year: string, month: string, day: string } {
  const from = filter.from ?? ''; const to = filter.to ?? ''
  const year = /^\d{4}-\d{2}-\d{2}$/.test(from) ? from.slice(0, 4) : ''
  const month = from.slice(5, 7); const day = from.slice(8, 10)
  if (!year || from === `${year}-01-01` && to === `${year}-12-31`) return { year, month: '', day: '' }
  if (to === `${year}-${month}-${String(daysInMonth(year, month)).padStart(2, '0')}` && day === '01') return { year, month, day: '' }
  return { year, month, day }
}
function daysInMonth(year: string, month: string): number { return new Date(Date.UTC(Number(year), Number(month), 0)).getUTCDate() }

function availableValues(part: Part, selected: { year: string, month: string }, dates: string[]): string[] {
  const validDates = dates.filter((date) => /^\d{4}-\d{2}-\d{2}$/.test(date))
  if (part === 'year') return distinct(validDates.map((date) => date.slice(0, 4)))
  if (part === 'month') return distinct(validDates
    .filter((date) => date.startsWith(`${selected.year}-`))
    .map((date) => date.slice(5, 7)))
  return distinct(validDates
    .filter((date) => date.startsWith(`${selected.year}-${selected.month}-`))
    .map((date) => date.slice(8, 10)))
}

function distinct(values: string[]): string[] {
  return [...new Set(values)].sort((left, right) => right.localeCompare(left))
}

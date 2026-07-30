import type { AlbumFilter } from '../../shared/albumPresentation'

export interface FilterBarEvents {
  onChange: (filter: AlbumFilter) => void
}

export function renderFilterBar(filter: AlbumFilter): string {
  const hasFilter = Boolean(filter.from || filter.to || filter.person)

  const year = filter.from ? filter.from.slice(0, 4) : ''
  const month = filter.from ? filter.from.slice(5, 7) : ''

  return `<div class="fan-filter-bar" id="fan-filter-bar">
    <span class="fan-filter-label">筛选</span>
    <button class="fan-filter-year ${year ? 'is-active' : ''}" id="fan-filter-year" type="button">
      ${year || '全部年份'} ▾
    </button>
    <button class="fan-filter-month ${month ? 'is-active' : ''}" id="fan-filter-month" type="button">
      ${month ? `${month} 月` : '全部月份'} ▾
    </button>
    ${hasFilter ? '<button class="fan-filter-clear" id="fan-filter-clear" type="button">清空筛选</button>' : ''}
  </div>`
}

export function mountFilterBar(
  container: HTMLElement,
  current: AlbumFilter,
  events: FilterBarEvents,
): () => void {
  container.innerHTML = renderFilterBar(current)

  const yearBtn = container.querySelector<HTMLButtonElement>('#fan-filter-year')
  const monthBtn = container.querySelector<HTMLButtonElement>('#fan-filter-month')
  const clearBtn = container.querySelector<HTMLButtonElement>('#fan-filter-clear')

  const onYearClick = () => events.onChange(toggleYear(current))
  const onMonthClick = () => events.onChange(toggleMonth(current))
  const onClear = () => events.onChange({})

  yearBtn?.addEventListener('click', onYearClick)
  monthBtn?.addEventListener('click', onMonthClick)
  clearBtn?.addEventListener('click', onClear)

  return () => {
    yearBtn?.removeEventListener('click', onYearClick)
    monthBtn?.removeEventListener('click', onMonthClick)
    clearBtn?.removeEventListener('click', onClear)
  }
}

function toggleYear(filter: AlbumFilter): AlbumFilter {
  const current = filter.from?.slice(0, 4)
  const next = current ? String(Number(current) + (current < '2026' ? 1 : -current.length)) : '2024'
  return { ...filter, from: `${next}-01-01`, to: `${next}-12-31` }
}

function toggleMonth(filter: AlbumFilter): AlbumFilter {
  if (!filter.from) return { ...filter, from: '2024-01-01', to: '2024-01-31' }
  const year = filter.from.slice(0, 4)
  const current = Number(filter.from.slice(5, 7))
  const next = current >= 12 ? 1 : current + 1
  const m = String(next).padStart(2, '0')
  return { ...filter, from: `${year}-${m}-01`, to: `${year}-${m}-31` }
}

import { api } from '../shared/api'
import { router } from '../shared/router'
import type { Page } from '../shared/router'
import type { ExifData, Photo } from '../shared/types'
import './detail.scss'

interface DetailPageParams {
  album: string
  photoId: number
}

export interface SwipeGesture {
  startX: number
  endX: number
  startY: number
  endY: number
}

export function getSwipeTargetId(gesture: SwipeGesture, photoId: number, total: number): number | null {
  const dx = gesture.endX - gesture.startX
  const dy = gesture.endY - gesture.startY
  const absX = Math.abs(dx)
  const absY = Math.abs(dy)

  if (absX < 48 || absX < absY * 1.25) return null
  if (dx < 0) return photoId < total - 1 ? photoId + 1 : null
  return photoId > 0 ? photoId - 1 : null
}

export function shouldBlockNativeDetailNavigation(gesture: SwipeGesture, viewportWidth: number): boolean {
  const edgeWidth = Math.max(24, Math.min(40, viewportWidth * 0.08))
  const startsAtHorizontalEdge = gesture.startX <= edgeWidth || gesture.startX >= viewportWidth - edgeWidth
  if (!startsAtHorizontalEdge) return false

  const dx = Math.abs(gesture.endX - gesture.startX)
  const dy = Math.abs(gesture.endY - gesture.startY)
  return dx >= 4 && dx > dy * 1.2
}
export async function createDetailPage({ album, photoId }: DetailPageParams): Promise<Page> {
  let albumData = await api.album(album)
  const photo = albumData.photos[photoId]
  let exif: ExifData | null = null

  if (photo) {
    try {
      exif = await api.exif(album, photo)
    } catch {
      // EXIF may not be available
    }
  }

  let keyHandler: ((e: KeyboardEvent) => void) | null = null

  return {
    async mount(container: HTMLElement) {
      if (!photo) {
        container.innerHTML = `<div class="error-state">
          <p>照片未找到</p>
          <button class="retry-btn" id="detail-back-album">返回相册</button>
        </div>`
        document.getElementById('detail-back-album')?.addEventListener('click', () => {
          router.navigate({ page: 'album', name: album })
        })
        return
      }

      const prevId = photoId > 0 ? photoId - 1 : null
      const nextId = photoId < albumData.photos.length - 1 ? photoId + 1 : null

      container.innerHTML = `
        <button class="back-btn" id="detail-back">← 返回相册</button>
        <div class="detail-page">
          <div class="detail-nav">
            ${prevId !== null
              ? `<button class="nav-btn" id="detail-prev" title="上一张">←</button>`
              : `<span class="nav-btn disabled">←</span>`}
            <span class="detail-pos">${photoId + 1} / ${albumData.photos.length}</span>
            ${nextId !== null
              ? `<button class="nav-btn" id="detail-next" title="下一张">→</button>`
              : `<span class="nav-btn disabled">→</span>`}
          </div>
          <div class="detail-main">
            <div class="detail-photo">
              <img
                src="${api.photoUrl(album, photo)}"
                alt="${escapeHtml(photo.name)}"
                loading="eager"
              />
              ${renderOriginalDownloadLink(album, photo)}
            </div>
            <aside class="detail-sidebar">
              <h3>照片信息</h3>
              ${renderPhotoInfo(exif, photo)}
            </aside>
          </div>
        </div>
      `

      // Navigation
      document.getElementById('detail-back')?.addEventListener('click', () => {
        router.navigate({ page: 'album', name: album })
      })
      document.getElementById('detail-prev')?.addEventListener('click', () => {
        router.replace({ page: 'detail', album, photoId: prevId! })
      })
      document.getElementById('detail-next')?.addEventListener('click', () => {
        router.replace({ page: 'detail', album, photoId: nextId! })
      })

      let swipeStart: { x: number; y: number } | null = null
      let horizontalSwipe = false
      const startSwipe = (x: number, y: number) => {
        swipeStart = { x, y }
        horizontalSwipe = false
      }
      const updateSwipe = (x: number, y: number) => {
        if (!swipeStart) return false
        const gesture = { startX: swipeStart.x, startY: swipeStart.y, endX: x, endY: y }
        const dx = Math.abs(x - swipeStart.x)
        const dy = Math.abs(y - swipeStart.y)
        horizontalSwipe = shouldBlockNativeDetailNavigation(gesture, window.innerWidth) || (dx > 12 && dx > dy * 1.1)
        return horizontalSwipe
      }
      const finishSwipe = (x: number, y: number) => {
        if (!swipeStart) return
        const targetId = getSwipeTargetId(
          { startX: swipeStart.x, startY: swipeStart.y, endX: x, endY: y },
          photoId,
          albumData.photos.length,
        )
        swipeStart = null
        horizontalSwipe = false
        if (targetId !== null) router.replace({ page: 'detail', album, photoId: targetId })
      }
      const detailPage = container.querySelector<HTMLElement>('.detail-page')
      detailPage?.addEventListener('pointerdown', (event) => {
        if (event.pointerType !== 'mouse' || event.button !== 0) return
        startSwipe(event.clientX, event.clientY)
      })
      detailPage?.addEventListener('pointerup', (event) => {
        if (event.pointerType !== 'mouse') return
        finishSwipe(event.clientX, event.clientY)
      })
      detailPage?.addEventListener('pointercancel', () => {
        swipeStart = null
        horizontalSwipe = false
      })
      detailPage?.addEventListener('touchstart', (event) => {
        const touch = event.touches[0]
        if (touch) startSwipe(touch.clientX, touch.clientY)
      }, { passive: true })
      detailPage?.addEventListener('touchmove', (event) => {
        const touch = event.touches[0]
        if (touch && updateSwipe(touch.clientX, touch.clientY)) event.preventDefault()
      }, { passive: false })
      detailPage?.addEventListener('touchend', (event) => {
        const touch = event.changedTouches[0]
        if (!touch) return
        if (horizontalSwipe) event.preventDefault()
        finishSwipe(touch.clientX, touch.clientY)
      }, { passive: false })
      detailPage?.addEventListener('touchcancel', () => {
        swipeStart = null
        horizontalSwipe = false
      })

      keyHandler = (e: KeyboardEvent) => {
        if (e.key === 'ArrowLeft' && prevId !== null) {
          router.replace({ page: 'detail', album, photoId: prevId })
        } else if (e.key === 'ArrowRight' && nextId !== null) {
          router.replace({ page: 'detail', album, photoId: nextId })
        } else if (e.key === 'Escape') {
          router.navigate({ page: 'album', name: album })
        }
      }
      document.addEventListener('keydown', keyHandler)
    },

    unmount() {
      if (keyHandler) document.removeEventListener('keydown', keyHandler)
      keyHandler = null
    },
  }
}

export function renderOriginalDownloadLink(albumId: string, photo: PhotoInfo): string {
  return `<a class="original-download-link" href="${api.photoUrl(albumId, photo)}?download=1" download="${escapeAttr(photo.name)}" aria-label="下载原图 ${escapeAttr(photo.name)}">下载原图</a>`
}

type PhotoInfo = Pick<Photo, 'id' | 'name' | 'width' | 'height' | 'size_bytes' | 'format'>
type InfoRow = [string, string | undefined | null]

export function renderPhotoInfo(exif: ExifData | null, photo: PhotoInfo): string {
  const dimensions = exif?.dimensions ?? (photo.width > 0 && photo.height > 0
    ? { width: photo.width, height: photo.height }
    : null)
  const fileSize = exif?.file_size ?? photo.size_bytes
  const format = exif?.format || photo.format

  const captureDevice = exif?.make && exif.model ? `${exif.make} ${exif.model}` : exif?.make || exif?.model
  const captureAddress = exif?.gps ? `${exif.gps.lat.toFixed(4)}, ${exif.gps.lon.toFixed(4)}` : null

  const sections = [
    renderInfoSection('基本信息', [
      ['文件名', photo.name],
      ['格式', format],
      ['尺寸', dimensions ? `${dimensions.width} × ${dimensions.height}` : null],
      ['像素', dimensions ? formatMegapixels(dimensions.width, dimensions.height) : null],
      ['文件大小', formatSize(fileSize)],
      ['拍摄日期', exif?.date_taken],
      ['时区', exif?.timezone],
      ['色彩空间', exif?.color_space],
    ]),
    renderInfoSection('拍摄信息', [
      ['拍摄设备', captureDevice],
      ['拍摄地址', captureAddress],
      ['拍摄时间', exif?.date_taken],
    ]),
    renderInfoSection('设备信息', [
      ['相机', captureDevice],
      ['镜头', exif?.lens],
      ['软件', exif?.software],
      ['作者', exif?.artist],
    ]),
    renderInfoSection('拍摄参数', [
      ['焦距', exif?.focal_length],
      ['光圈', exif?.aperture],
      ['快门', exif?.shutter_speed],
      ['ISO', exif?.iso == null ? null : `ISO ${exif.iso}`],
      ['闪光灯', exif?.flash],
      ['方向', exif?.orientation == null ? null : String(exif.orientation)],
      ['GPS', captureAddress],
    ]),
    renderInfoSection('内容备注', [
      ['标题', exif?.image_description],
      ['备注', exif?.user_comment],
    ]),
    renderTags(exif?.tags ?? []),
    renderInfoSection('影调分析', [
      ['影调', exif?.tone?.type],
      ['亮度', exif?.tone ? `${exif.tone.brightness}%` : null],
      ['对比度', exif?.tone ? `${exif.tone.contrast}%` : null],
      ['阴影', exif?.tone ? `${exif.tone.shadows}%` : null],
      ['高光', exif?.tone ? `${exif.tone.highlights}%` : null],
    ]),
  ]

  return sections.filter(Boolean).join('')
}

function renderInfoSection(title: string, rows: InfoRow[]): string {
  const body = rows
    .filter(([, value]) => value)
    .map(([label, value]) => `<div class="exif-row">
      <span class="exif-label">${label}</span>
      <span class="exif-value">${escapeHtml(value!)}</span>
    </div>`)
    .join('')

  if (!body) return ''

  return `<section class="exif-section">
    <h4>${title}</h4>
    ${body}
  </section>`
}

function renderTags(tags: string[]): string {
  const visibleTags = tags.filter(Boolean).slice(0, 24)
  if (visibleTags.length === 0) return ''

  return `<section class="exif-section">
    <h4>标签</h4>
    <div class="exif-tags">
      ${visibleTags.map((tag) => `<span class="exif-tag">${escapeHtml(tag)}</span>`).join('')}
    </div>
  </section>`
}

function formatMegapixels(width: number, height: number): string {
  const megapixels = Math.floor((width * height) / 1_000_000)
  return `${Math.max(1, megapixels)} MP`
}

function formatSize(bytes: number): string {
  if (bytes > 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`
  if (bytes > 1024) return `${(bytes / 1024).toFixed(0)} KB`
  return `${bytes} B`
}

function escapeHtml(s: string): string {
  return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
}

function escapeAttr(s: string): string {
  return escapeHtml(s).replace(/"/g, '&quot;')
}

import * as THREE from 'three'
import type { Album } from '../../shared/types'
import { api } from '../../shared/api'
import { albumPresentation } from '../../shared/albumPresentation'
import { lerp } from '../../shared/math'
import { getActiveFanIndex, getFanAlbumAtScreenPoint, getFanCardMetrics, getFanCardPose, getFanPosterStats, isFanDrag, isFanGestureStart } from './fanLayout'

// Three injects built-in attributes/uniforms for ShaderMaterial; only declare
// custom uniforms/varyings here.
const vertexShader = /* glsl */ `
precision highp float;
uniform float uTime;
uniform float uSpeed;
varying vec2 vUv;
void main() {
  vUv = uv;
  vec3 p = position;
  p.z = (sin(p.x * 4.0 + uTime) * 1.5 + cos(p.y * 2.0 + uTime) * 1.5) * (0.1 + uSpeed * 0.5);
  gl_Position = projectionMatrix * modelViewMatrix * vec4(p, 1.0);
}
`

const fragmentShader = /* glsl */ `
precision highp float;
uniform vec2 uImageSizes;
uniform vec2 uPlaneSizes;
uniform sampler2D tMap;
varying vec2 vUv;
void main() {
  vec2 ratio = vec2(
    min((uPlaneSizes.x / uPlaneSizes.y) / (uImageSizes.x / uImageSizes.y), 1.0),
    min((uPlaneSizes.y / uPlaneSizes.x) / (uImageSizes.y / uImageSizes.x), 1.0)
  );
  vec2 uv = vec2(
    vUv.x * ratio.x + (1.0 - ratio.x) * 0.5,
    vUv.y * ratio.y + (1.0 - ratio.y) * 0.5
  );
  gl_FragColor.rgb = texture2D(tMap, uv).rgb;
  gl_FragColor.a = 1.0;
}
`

function makePlaceholder(): THREE.Texture {
  const c = document.createElement('canvas')
  c.width = c.height = 4
  const ctx = c.getContext('2d')!
  ctx.fillStyle = '#141a21'
  ctx.fillRect(0, 0, 4, 4)
  const t = new THREE.CanvasTexture(c)
  t.minFilter = THREE.LinearFilter
  t.colorSpace = THREE.SRGBColorSpace
  return t
}

// Home gallery: bounded horizontal arc inspired by the circular WebGL reference,
// adjusted for small real album counts so cards stay on screen.

export class FanScene {
  container: HTMLElement
  onAlbumClick?: (album: Album) => void

  private renderer!: THREE.WebGLRenderer
  private camera!: THREE.PerspectiveCamera
  private scene!: THREE.Scene
  private cards: Card[] = []

  private scroll = { ease: 0.05, current: 0, target: 0, last: 0 }
  private direction: 'left' | 'right' = 'right'
  private screen = { width: 0, height: 0 }
  private viewport = { width: 0, height: 0 }
  private isDown = false
  private startX = 0
  private scrollPos = 0
  private rafId = 0
  private observer: ResizeObserver | null = null
  private checkTimer: ReturnType<typeof setTimeout> | undefined
  private captionLayer!: HTMLElement
  private posterChrome!: HTMLElement
  private activeAlbumIndex = -1
  private gesturePointer = -1
  private didDrag = false

  constructor(container: HTMLElement) {
    this.container = container
    this.setup()
  }

  private setup() {
    const w = window.innerWidth
    const h = window.innerHeight

    this.renderer = new THREE.WebGLRenderer({ antialias: true, alpha: true, preserveDrawingBuffer: true })
    this.renderer.setPixelRatio(Math.min(window.devicePixelRatio, 2))
    this.renderer.setSize(w, h)
    this.renderer.setClearColor(0x000000, 0)
    this.container.classList.add('fan-page')
    this.container.appendChild(this.renderer.domElement)
    this.captionLayer = document.createElement('div')
    this.captionLayer.className = 'fan-caption-layer'
    this.container.appendChild(this.captionLayer)
    this.posterChrome = document.createElement('div')
    this.posterChrome.className = 'fan-poster-chrome'
    this.container.appendChild(this.posterChrome)

    this.camera = new THREE.PerspectiveCamera(45, w / h, 0.1, 100)
    this.camera.position.z = 20

    this.scene = new THREE.Scene()

    this.container.addEventListener('wheel', this.onWheel, { passive: false })
    this.container.addEventListener('pointerdown', this.onDown)
    window.addEventListener('pointermove', this.onMove)
    window.addEventListener('pointerup', this.onUp)
    window.addEventListener('pointercancel', this.onUp)

    this.observer = new ResizeObserver(() => this.onResize())
    this.observer.observe(this.container)

    this.onResize()
    this.update()
  }

  setAlbums(albums: Album[]) {
    this.cards.forEach((c) => { this.scene.remove(c.mesh); c.dispose() })
    this.cards = []

    this.captionLayer.innerHTML = ''
    this.posterChrome.innerHTML = ''
    this.activeAlbumIndex = -1
    const total = albums.length
    for (let i = 0; i < total; i++) {
      const album = albums[i]
      const card = new Card(album, i, total)
      this.scene.add(card.mesh)
      this.cards.push(card)
    }

    this.onResize()
  }

  // ---- Resize ----
  private onResize = () => {
    const rect = this.container.getBoundingClientRect()
    this.screen = { width: rect.width, height: rect.height }
    this.renderer.setSize(this.screen.width, this.screen.height)
    this.camera.aspect = this.screen.width / this.screen.height
    this.camera.updateProjectionMatrix()

    const fov = this.camera.fov * (Math.PI / 180)
    const height = 2 * Math.tan(fov / 2) * this.camera.position.z
    const width = height * this.camera.aspect
    this.viewport = { height, width }

    for (const card of this.cards) card.onResize(this.screen, this.viewport)
    for (let i = 0; i < this.cards.length; i++) this.cards[i].setLayout(i, this.cards.length)
  }

  // ---- Update ----
  private update = () => {
    this.rafId = requestAnimationFrame(this.update)
    this.scroll.current = lerp(this.scroll.current, this.scroll.target, this.scroll.ease)
    this.direction = this.scroll.current > this.scroll.last ? 'right' : 'left'
    for (const card of this.cards) card.update(this.scroll, this.direction, this.viewport)
    this.syncPosterChrome()
    this.syncActiveCaption()
    this.renderer.render(this.scene, this.camera)
    this.scroll.last = this.scroll.current
  }

  // ---- Events ----
  private onWheel = (e: WheelEvent) => {
    e.preventDefault()
    this.scroll.target += e.deltaY * 0.018
    clearTimeout(this.checkTimer)
    this.checkTimer = setTimeout(() => this.onCheck(), 200)
  }

  private onDown = (e: PointerEvent) => {
    if (e.button !== 0) return
    if (e.target instanceof Element && e.target.closest('.fan-film-rail')) return
    if (this.gesturePointer !== -1) return
    this.gesturePointer = e.pointerId

    this.isDown = true
    this.didDrag = false
    this.scrollPos = this.scroll.current
    this.startX = e.clientX
    this.container.classList.add('is-dragging')
  }

  private onMove = (e: PointerEvent) => {
    if (!this.isDown) return
    if (e.pointerId !== this.gesturePointer) return
    const distance = (this.startX - e.clientX) * 0.032
    if (isFanDrag(this.startX, e.clientX)) this.didDrag = true
    this.scroll.target = this.scrollPos + distance
  }

  private onUp = (e: PointerEvent) => {
    if (e.pointerId !== this.gesturePointer) return
    this.gesturePointer = -1

    if (!this.isDown) return

    this.isDown = false
    this.container.classList.remove('is-dragging')
    this.onCheck()

    if (!this.didDrag && e.type !== 'pointercancel') {
      const rect = this.renderer.domElement.getBoundingClientRect()
      this.scene.updateMatrixWorld()
      const album = getFanAlbumAtScreenPoint(this.camera, this.cards, rect, e.clientX, e.clientY)
      if (album) this.onAlbumClick?.(album)
    }

    window.setTimeout(() => { this.didDrag = false }, 0)
  }
  private onCheck = () => {
    if (this.cards.length === 0) return
    const w = this.cards[0].width
    const idx = Math.round(Math.abs(this.scroll.target) / w)
    this.scroll.target = this.scroll.target < 0 ? -(w * idx) : w * idx
  }


  private syncPosterChrome() {
    const active = getActiveFanIndex(this.cards.map((card) => card.mesh.position.x))
    if (active === this.activeAlbumIndex) return

    this.activeAlbumIndex = active
    const card = this.cards[active]
    if (!card) {
      this.posterChrome.innerHTML = ''
      return
    }

    const presentation = albumPresentation(card.album)
    const stats = getFanPosterStats(this.cards.map((item) => albumPhotoCount(item.album)))
    this.cards.forEach((item) => { item.caption = null })
    this.captionLayer.innerHTML = `
      <div class="fan-photo-caption" aria-hidden="true">
        ${presentation.metadata ? `<div class="fan-photo-metadata">${escapeHtml(presentation.metadata)}</div>` : ''}
        <div class="fan-photo-summary">${escapeHtml(presentation.summary)}</div>
      </div>
    `
    const caption = this.captionLayer.firstElementChild
    if (caption instanceof HTMLElement) card.caption = caption

    this.posterChrome.innerHTML = `
      <div class="fan-poster-top">
        <div>
          <div class="fan-poster-kicker">LumiFlow Albums</div>
          <div class="fan-poster-title">${escapeHtml(presentation.metadata)}</div>
          <div class="fan-poster-description">${escapeHtml(presentation.summary)}</div>
        </div>
        <div class="fan-poster-stats"><span>${stats.albumCount}</span> albums<br><span>${stats.photoCount}</span> photos</div>
      </div>
      <div class="fan-poster-bottom-caption">${active + 1} / ${this.cards.length} · 左右滑动浏览 · 轻点照片进入</div>
      <div class="fan-film-rail" aria-label="相册缩略图">
        ${this.cards.map((item, i) => `<button class="fan-film-frame ${i === active ? 'is-active' : ''}" type="button" aria-label="切换到 ${escapeAttr(item.album.name)}"><img src="${albumCoverThumbUrl(item.album)}" alt=""><span>${i + 1}</span></button>`).join('')}
      </div>
    `

    this.posterChrome.querySelectorAll<HTMLButtonElement>('.fan-film-frame').forEach((button, i) => {
      button.addEventListener('click', () => {
        if (this.didDrag) return
        const target = this.cards[i]
        if (target) this.scroll.target = target.x - target.extra
      })
    })
  }
  private syncActiveCaption() {
    this.cards[this.activeAlbumIndex]?.syncCaption(this.viewport)
  }

  dispose() {
    cancelAnimationFrame(this.rafId)
    clearTimeout(this.checkTimer)
    this.observer?.disconnect()
    this.container.removeEventListener('wheel', this.onWheel)
    this.container.removeEventListener('pointerdown', this.onDown)
    window.removeEventListener('pointermove', this.onMove)
    window.removeEventListener('pointerup', this.onUp)
    window.removeEventListener('pointercancel', this.onUp)
    this.cards.forEach((c) => c.dispose())
    this.container.classList.remove('fan-page', 'is-dragging')
    this.captionLayer.remove()
    this.posterChrome.remove()
    this.renderer.dispose()
    this.renderer.domElement.remove()
  }
}

// ---- Card ----
class Card {
  mesh: THREE.Mesh
  album: Album
  program: THREE.ShaderMaterial
  caption: HTMLElement | null = null

  extra = 0
  x = 0
  width = 0
  widthTotal = 0
  isBefore = false
  isAfter = false

  constructor(album: Album, index: number, total: number) {
    this.album = album

    const placeholder = makePlaceholder()

    // PlaneGeometry with segments for vertex shader wave
    const geo = new THREE.PlaneGeometry(1, 1, 50, 100)

    this.program = new THREE.ShaderMaterial({
      vertexShader,
      fragmentShader,
      uniforms: {
        tMap: { value: placeholder },
        uImageSizes: { value: [4, 4] },
        uPlaneSizes: { value: [1, 1] },
        uTime: { value: 0 },
        uSpeed: { value: 0 },
      },
    })

    this.mesh = new THREE.Mesh(geo, this.program)

    // Async load real texture
    const loader = new THREE.TextureLoader()
    loader.load(
      albumCoverThumbUrl(album),
      (tex: THREE.Texture) => {
        tex.minFilter = THREE.LinearFilter
        tex.generateMipmaps = false
        tex.colorSpace = THREE.SRGBColorSpace
        const previous = this.program.uniforms.tMap.value as THREE.Texture | undefined
        this.program.uniforms.tMap.value = tex
        if (previous && previous !== tex) previous.dispose()
        if (tex.image) {
          const image = tex.image as HTMLImageElement | HTMLCanvasElement
          const width = 'naturalWidth' in image ? image.naturalWidth : image.width
          const height = 'naturalHeight' in image ? image.naturalHeight : image.height
          this.program.uniforms.uImageSizes.value = [width || 1, height || 1]
        }
      },
    )
  }

  onResize(screen: { width: number; height: number }, viewport: { width: number; height: number }) {
    const metrics = getFanCardMetrics(screen, viewport)
    this.mesh.scale.y = metrics.height
    this.mesh.scale.x = metrics.width
    this.program.uniforms.uPlaneSizes.value = [this.mesh.scale.x, this.mesh.scale.y]
    this.width = metrics.slot
  }

  setLayout(index: number, total: number) {
    this.widthTotal = this.width * total
    this.x = this.width * index
  }

  update(
    scroll: { current: number; last: number },
    direction: 'left' | 'right',
    viewport: { width: number; height: number },
  ) {
    this.mesh.position.x = this.x - scroll.current - this.extra
    const pose = getFanCardPose(this.mesh.position.x, viewport.width, viewport.height)
    this.mesh.position.y = pose.y
    this.mesh.rotation.z = pose.rotationZ

    const speed = scroll.current - scroll.last
    this.program.uniforms.uTime.value += 0.04
    this.program.uniforms.uSpeed.value = speed

    const planeOffset = this.mesh.scale.x / 2
    this.isBefore = this.mesh.position.x + planeOffset < -viewport.width * 0.62
    this.isAfter = this.mesh.position.x - planeOffset > viewport.width * 0.62

    if (direction === 'right' && this.isBefore) {
      this.extra -= this.widthTotal
    }
    if (direction === 'left' && this.isAfter) {
      this.extra += this.widthTotal
    }

  }

  syncCaption(viewport: { width: number; height?: number }) {
    if (!this.caption) return
    const x = ((this.mesh.position.x / viewport.width) + 0.5) * 100
    const isPortrait = (viewport.height ?? 0) > viewport.width * 1.35
    const centerY = isPortrait
      ? 70 - (this.mesh.position.y + 3.85) * 10
      : 50 - (this.mesh.position.y / (viewport.height ?? 1)) * 100
    const cardHeight = (this.mesh.scale.y / Math.max(viewport.height ?? 1, 1)) * 100
    const y = centerY + cardHeight * (isPortrait ? 0.3 : 0.36)
    const scale = Math.max(isPortrait ? 0.86 : 0.78, Math.min(1.08, 1 - Math.abs(this.mesh.position.x / viewport.width) * (isPortrait ? 0.12 : 0.22)))
    this.caption.style.transform = `translate(-50%, -100%) translate(${x}vw, ${y}vh) rotate(${this.mesh.rotation.z}rad) scale(${scale})`
    this.caption.style.opacity = `${Math.max(0.28, Math.min(1, 1 - Math.abs(this.mesh.position.x / viewport.width) * 0.75))}`
  }

  dispose() {
    this.caption?.remove()
    this.program.uniforms.tMap.value?.dispose()
    this.program.dispose()
    this.mesh.geometry.dispose()
  }
}

function albumPhotoCount(album: Album): number {
  return album.photo_count ?? album.count ?? 0
}

function albumCoverThumbUrl(album: Album): string {
  const coverPhotoId = album.cover_photo_id
  if (coverPhotoId) {
    return api.thumbUrl(album.id ?? album.name, { id: coverPhotoId, name: coverPhotoId })
  }

  return api.thumbUrl(album.name, { id: 0, name: album.cover ?? '' })
}

function escapeHtml(s: string): string {
  return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
}

function escapeAttr(s: string): string {
  return escapeHtml(s).replace(/"/g, '&quot;')
}

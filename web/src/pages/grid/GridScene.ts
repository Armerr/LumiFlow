import * as THREE from 'three'
import gsap from 'gsap'
import type { Photo } from '../../shared/types'
import { api } from '../../shared/api'
import { buildInfiniteGridItems, getContainedPhotoSize, getGridColumnCount, getGridFieldSize, getGridMotionMultiplier, getGridPlaneInset, REFERENCE_GRID_ITEM_COUNT, type InfiniteGridItem } from './gridLayout'
// Shader pair mirrors the draggable WebGL grid reference: DOM-measured planes,
// cover-fit UVs, and drag-diff geometry compression.

const vertexShader = /* glsl */ `
precision mediump float;
uniform float u_diff;
varying vec2 vUv;
void main(){
  vec3 pos = position;
  pos.y *= 1. - u_diff;
  pos.x *= 1. - u_diff;
  vUv = uv;
  gl_Position = projectionMatrix * modelViewMatrix * vec4(pos, 1.);
}
`

const fragmentShader = /* glsl */ `
precision mediump float;
uniform vec2 u_res;
uniform vec2 u_size;
uniform sampler2D u_texture;
vec2 cover(vec2 screenSize, vec2 imageSize, vec2 uv) {
  float screenRatio = screenSize.x / screenSize.y;
  float imageRatio = imageSize.x / imageSize.y;
  vec2 newSize = screenRatio < imageRatio
      ? vec2(imageSize.x * (screenSize.y / imageSize.y), screenSize.y)
      : vec2(screenSize.x, imageSize.y * (screenSize.x / imageSize.x));
  vec2 newOffset = (screenRatio < imageRatio
      ? vec2((newSize.x - screenSize.x) / 2.0, 0.0)
      : vec2(0.0, (newSize.y - screenSize.y) / 2.0)) / newSize;
  return uv * screenSize / newSize + newOffset;
}
varying vec2 vUv;
void main() {
    vec2 uvCover = cover(u_res, u_size, vUv);
    gl_FragColor = texture2D(u_texture, uvCover);
}
`

// ---- Base geometry & material (cloned per plane) ----

const geometry = new THREE.PlaneGeometry(1, 1, 1, 1)
const baseMaterial = new THREE.ShaderMaterial({
  vertexShader,
  fragmentShader,
})

const loader = new THREE.TextureLoader()

interface LegacyWheelEvent extends WheelEvent {
  readonly wheelDeltaX?: number
  readonly wheelDeltaY?: number
}

function makePlaceholder(): THREE.Texture {
  const c = document.createElement('canvas')
  c.width = c.height = 4
  const ctx = c.getContext('2d')!
  ctx.fillStyle = '#2a2a2a'
  ctx.fillRect(0, 0, 4, 4)
  const t = new THREE.CanvasTexture(c)
  t.minFilter = THREE.LinearFilter
  t.colorSpace = THREE.SRGBColorSpace
  return t
}

// ---- Plane (exact port of reference Plane class) ----

class Plane extends THREE.Object3D {
  el!: HTMLElement
  material!: THREE.ShaderMaterial
  mesh!: THREE.Mesh
  rect!: DOMRect
  xOffset = 0
  yOffset = 0
  index = 0
  my = 0
  photo: Photo | null = null
  inset = 0

  init(el: HTMLElement, i: number, columns: number) {
    this.el = el
    this.index = i
    this.setColumnCount(columns)
    this.material = baseMaterial.clone()
    this.material.uniforms = {
      u_texture: { value: makePlaceholder() },
      u_res: { value: new THREE.Vector2(1, 1) },
      u_size: { value: new THREE.Vector2(4, 4) },
      u_diff: { value: 0 },
    }
    this.mesh = new THREE.Mesh(geometry, this.material)
    this.add(this.mesh)
  }


  setColumnCount(columns: number) {
    this.my = getGridMotionMultiplier(this.index, columns)
  }
  loadTexture(album: string, photo: Photo) {
    this.photo = photo
    loader.load(api.thumbUrl(album, photo.name), (texture) => {
      texture.minFilter = THREE.LinearFilter
      texture.generateMipmaps = false
      texture.colorSpace = THREE.SRGBColorSpace

      const previous = this.material.uniforms.u_texture.value as THREE.Texture | undefined
      this.material.uniforms.u_texture.value = texture

      if (previous && previous !== texture) previous.dispose()

      if (texture.image) {
        const image = texture.image as HTMLImageElement | HTMLCanvasElement
        const width = 'naturalWidth' in image ? image.naturalWidth : image.width
        const height = 'naturalHeight' in image ? image.naturalHeight : image.height
        this.material.uniforms.u_size.value.set(width || 1, height || 1)
        if (width > 0 && height > 0) {
          this.syncElementSize({ ...photo, width, height }, this.inset)
          this.resize()
        }
      }
    })
  }


  syncElementSize(photo: Photo, inset: number) {
    const cell = this.el.parentElement?.getBoundingClientRect()
    if (!cell) return

    this.inset = inset
    const width = photo.width || this.photo?.width || 1
    const height = photo.height || this.photo?.height || 1
    this.photo = { ...photo, width, height }
    const size = getContainedPhotoSize(
      { width, height },
      { width: cell.width, height: cell.height },
      inset,
    )
    this.el.style.width = `${size.width}px`
    this.el.style.height = `${size.height}px`
  }

  update(x: number, y: number, max: { x: number; y: number }, diff: number) {
    const { right, bottom } = this.rect
    this.position.y = gsap.utils.wrap(-(max.y - bottom), bottom, y * this.my) - this.yOffset
    this.position.x = gsap.utils.wrap(-(max.x - right), right, x) - this.xOffset
    this.material.uniforms.u_diff.value = diff
  }

  resize() {
    this.rect = this.el.getBoundingClientRect()
    const { left, top, width, height } = this.rect
    this.xOffset = left + width / 2 - window.innerWidth / 2
    this.yOffset = top + height / 2 - window.innerHeight / 2
    this.position.x = this.xOffset
    this.position.y = this.yOffset
    this.material.uniforms.u_res.value.set(Math.max(width, 1), Math.max(height, 1))
    this.mesh.scale.set(Math.max(width, 1), Math.max(height, 1), 1)
  }

  dispose() {
    this.material.uniforms.u_texture.value?.dispose()
    this.material.dispose()
  }
}

// ---- Core (exact port of reference Core class) ----

export class GridScene {
  container: HTMLElement
  onPhotoClick?: (index: number) => void

  private tx = 0
  private ty = 0
  private cx = 0
  private cy = 0
  private diff = 0
  private max = { x: 0, y: 0 }
  private isDragging = false
  private dragStart = { x: 0, y: 0 }
  private on = { x: 0, y: 0 }
  private suppressClick = false

  private scene!: THREE.Scene
  private camera!: THREE.OrthographicCamera
  private renderer!: THREE.WebGLRenderer
  private planes: Plane[] = []
  private gridEl!: HTMLElement
  private photos: Photo[] = []
  private gridItems: InfiniteGridItem[] = []
  private album = ''

  private resizeObserver: ResizeObserver | null = null

  constructor(container: HTMLElement) {
    this.container = container
  }

  init(album: string, photos: Photo[]) {
    this.album = album
    this.photos = photos
    this.gridItems = buildInfiniteGridItems(photos, REFERENCE_GRID_ITEM_COUNT)

    const w = window.innerWidth
    const h = window.innerHeight

    this.scene = new THREE.Scene()
    this.camera = new THREE.OrthographicCamera(w / -2, w / 2, h / 2, h / -2, 1, 1000)
    this.camera.position.z = 1

    this.renderer = new THREE.WebGLRenderer({ antialias: true, preserveDrawingBuffer: true })
    this.renderer.setSize(w, h)
    this.renderer.setPixelRatio(gsap.utils.clamp(1, 1.5, window.devicePixelRatio))
    this.renderer.setClearColor(0x000000, 0)
    this.renderer.domElement.className = 'grid-webgl-canvas'
    document.body.appendChild(this.renderer.domElement)

    this.gridEl = this.container.querySelector('.js-grid') as HTMLElement
    this.buildGrid()
    this.addPlanes()
    this.resize()

    gsap.ticker.add(this.tick)
    this.addEvents()


    this.resizeObserver = new ResizeObserver(() => this.resize())
    this.resizeObserver.observe(this.container)
  }

  private buildGrid() {
    this.gridEl.innerHTML = this.gridItems
      .map(
        (item) =>
          `<div><figure class="js-plane" data-key="${item.key}" data-index="${item.sourceIndex}" aria-label="${escapeAttr(item.photo.name)}"></figure></div>`,
      )
      .join('')

    this.gridEl.querySelectorAll<HTMLElement>('.js-plane').forEach((el, i) => {
      el.addEventListener('click', (event) => {
        if (this.suppressClick) {
          event.preventDefault()
          event.stopPropagation()
          return
        }
        this.onPhotoClick?.(this.gridItems[i].sourceIndex)
      })
    })
  }

  private addPlanes() {
    const els = this.gridEl.querySelectorAll<HTMLElement>('.js-plane')
    els.forEach((el, i) => {
      const plane = new Plane()
      plane.init(el, i, getGridColumnCount(window.innerWidth))
      plane.loadTexture(this.album, this.gridItems[i].photo)
      this.scene.add(plane)
      this.planes.push(plane)
    })
  }

  private addEvents() {
    window.addEventListener('pointermove', this.onPointerMove)
    window.addEventListener('pointerdown', this.onPointerDown)
    window.addEventListener('pointerup', this.onPointerUp)
    window.addEventListener('pointercancel', this.onPointerUp)
    window.addEventListener('wheel', this.onWheel, { passive: false })
    window.addEventListener('keydown', this.onKeyDown)
  }

  private tick = () => {
    const xDiff = this.tx - this.cx
    const yDiff = this.ty - this.cy
    this.cx += xDiff * 0.085
    this.cy += yDiff * 0.085
    this.cx = Math.round(this.cx * 100) / 100
    this.cy = Math.round(this.cy * 100) / 100
    this.diff = Math.max(Math.abs(yDiff * 0.0001), Math.abs(xDiff * 0.0001))
    this.planes.forEach((p) => p.update(this.cx, this.cy, this.max, this.diff))
    this.renderer.render(this.scene, this.camera)
  }

  private onPointerMove = (e: PointerEvent) => {
    if (!this.isDragging) return

    const dx = e.clientX - this.dragStart.x
    const dy = e.clientY - this.dragStart.y
    if (Math.hypot(dx, dy) > 6) this.suppressClick = true

    this.tx = this.on.x + e.clientX * 2.5
    this.ty = this.on.y - e.clientY * 2.5
  }

  private onPointerDown = (e: PointerEvent) => {
    if (e.button !== 0 || this.isDragging) return
    this.isDragging = true
    this.suppressClick = false
    this.dragStart.x = e.clientX
    this.dragStart.y = e.clientY
    this.on.x = this.tx - e.clientX * 2.5
    this.on.y = this.ty + e.clientY * 2.5
    this.container.classList.add('is-dragging')
  }

  private onPointerUp = () => {
    if (!this.isDragging) return
    this.isDragging = false
    this.container.classList.remove('is-dragging')
    if (this.suppressClick) window.setTimeout(() => { this.suppressClick = false }, 0)
  }

  private onWheel = (e: WheelEvent) => {
    e.preventDefault()
    const legacyWheel = e as LegacyWheelEvent
    const isFirefox = navigator.userAgent.includes('Firefox')
    const isWindows = navigator.userAgent.includes('Windows')
    const multiplier = isWindows ? 1.2 : 0.6
    const firefoxMultiplier = isWindows ? 40 : 20
    let wx = legacyWheel.wheelDeltaX ?? e.deltaX * -1
    let wy = legacyWheel.wheelDeltaY ?? e.deltaY * -1

    if (isFirefox && e.deltaMode === 1) {
      wx *= firefoxMultiplier
      wy *= firefoxMultiplier
    }

    this.tx += wx * multiplier
    this.ty -= wy * multiplier
  }

  private onKeyDown = (e: KeyboardEvent) => {
    if (e.key !== 'Escape') return
    this.tx = 0
    this.ty = 0
  }

  private resize = () => {
    const w = window.innerWidth
    const h = window.innerHeight
    this.renderer.setSize(w, h)
    this.camera.left = w / -2
    this.camera.right = w / 2
    this.camera.top = h / 2
    this.camera.bottom = h / -2
    this.camera.updateProjectionMatrix()

    const columns = getGridColumnCount(w)
    const rows = Math.ceil(this.gridItems.length / columns)
    const field = getGridFieldSize({ width: w, height: h })
    this.gridEl.style.setProperty('--grid-cols', `${columns}`)
    this.gridEl.style.setProperty('--grid-rows', `${rows}`)
    this.gridEl.style.setProperty('--grid-width', `${field.width}px`)
    this.gridEl.style.setProperty('--grid-height', `${field.height}px`)
    this.planes.forEach((p) => p.setColumnCount(columns))
    const inset = getGridPlaneInset(w)
    this.planes.forEach((p, i) => p.syncElementSize(this.gridItems[i].photo, inset))

    this.planes.forEach((p) => p.resize())
    const rect = this.gridEl.getBoundingClientRect()
    this.max.x = rect.right
    this.max.y = rect.bottom
  }

  dispose() {
    gsap.ticker.remove(this.tick)
    this.resizeObserver?.disconnect()
    window.removeEventListener('pointermove', this.onPointerMove)
    window.removeEventListener('pointerdown', this.onPointerDown)
    window.removeEventListener('pointerup', this.onPointerUp)
    window.removeEventListener('pointercancel', this.onPointerUp)
    window.removeEventListener('wheel', this.onWheel)
    window.removeEventListener('keydown', this.onKeyDown)
    this.container.classList.remove('is-dragging')
    this.planes.forEach((p) => p.dispose())
    this.planes = []
    this.renderer.dispose()
    this.renderer.domElement.remove()
  }
}

function escapeAttr(s: string): string {
  return s.replace(/&/g, '&amp;').replace(/"/g, '&quot;').replace(/</g, '&lt;')
}

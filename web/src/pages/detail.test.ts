import { afterEach, describe, expect, test, vi } from 'vitest'
import { createDetailPage, renderDetailPreview, renderOriginalDownloadLink, renderPhotoInfo } from './detail'

afterEach(() => {
  vi.unstubAllGlobals()
})


describe('detail data loading', () => {
  test('fetches only the requested photo metadata instead of the full album', async () => {
    const fetchMock = vi.fn()
      .mockResolvedValueOnce({
        ok: true,
        json: () => Promise.resolve({ name: 'Trip', photo_count: 4750, photos: [{ id: 'p42', name: 'photo.jpg', width: 10, height: 10, size_bytes: 1, format: 'JPG' }] }),
      })
      .mockResolvedValueOnce({ ok: true, json: () => Promise.resolve({}) })
    vi.stubGlobal('fetch', fetchMock)

    await createDetailPage({ album: 'day:2025-01-02', photoId: 42 })

    expect(fetchMock).toHaveBeenNthCalledWith(1, '/api/albums/day%3A2025-01-02?offset=42&limit=1')
  })
})

describe('detail preview', () => {
  test('uses a browser-compatible WebP preview for unsupported originals', () => {
    const html = renderDetailPreview('Trip', {
      id: 'p42',
      name: 'photo.HIF',
      width: 10,
      height: 10,
      size_bytes: 1,
      format: 'HIF',
    })

    expect(html).toContain('src="/api/thumbs/by-id/p42"')
    expect(html).toContain('alt="photo.HIF"')
  })
})

describe('detail original download', () => {
  test('renders same-origin original download link for mobile detail UI', () => {
    const html = renderOriginalDownloadLink('Test Album', {
      id: 0,
      name: 'DSCF6138.HIF',
      width: 5152,
      height: 7728,
      size_bytes: 8178892,
      format: 'HIF',
    })

    expect(html).toContain('class="original-download-link"')
    expect(html).toContain('href="/api/photos/Test%20Album/DSCF6138.HIF?download=1"')
    expect(html).toContain('download="DSCF6138.HIF"')
    expect(html).toContain('下载原图')
  })

  test('renders a by-ID original download link for timeline photos', () => {
    const html = renderOriginalDownloadLink('day:2026-06-02', {
      id: 'sha1-photo-id',
      name: 'DSCF6138.HIF',
      width: 5152,
      height: 7728,
      size_bytes: 8178892,
      format: 'HIF',
    })

    expect(html).toContain('href="/api/photos/by-id/sha1-photo-id?download=1"')
    expect(html).toContain('download="DSCF6138.HIF"')
  })
})

describe('photo metadata rendering', () => {
  test('groups useful basic, device, shooting, and tag fields', () => {
    const html = renderPhotoInfo({
      make: 'FUJIFILM',
      model: 'X-T5',
      lens: 'XF56mmF1.2 R WR',
      focal_length: '56mm',
      aperture: 'f/2.8',
      shutter_speed: '1/1250s',
      iso: 160,
      date_taken: '2026/6/2 11:56:45',
      timezone: 'UTC+9',
      gps: { lat: 35.6586, lon: 139.7454 },
      dimensions: { width: 5152, height: 7728 },
      file_size: 8178892,
      format: 'HIF',
      flash: null,
      software: 'Digital Camera X-T5 Ver4.31',
      orientation: 1,
      artist: 'INNEI',
      color_space: 'sRGB',
      image_description: null,
      user_comment: null,
      tags: ['日本', '东京', '浅草'],
    }, {
      id: 0,
      name: 'DSCF6138.HIF',
      width: 5152,
      height: 7728,
      size_bytes: 8178892,
      format: 'HIF',
    })

    expect(html).toContain('基本信息')
    expect(html).toContain('DSCF6138')
    expect(html).toContain('5152 × 7728')
    expect(html).toContain('39 MP')
    expect(html).toContain('UTC+9')
    expect(html).toContain('拍摄信息')
    expect(html).toContain('拍摄设备')
    expect(html).toContain('拍摄地址')
    expect(html).toContain('35.6586')
    expect(html).toContain('139.7454')
    expect(html).toContain('设备信息')
    expect(html).toContain('FUJIFILM X-T5')
    expect(html).toContain('拍摄参数')
    expect(html).toContain('56mm')
    expect(html).toContain('f/2.8')
    expect(html).toContain('1/1250s')
    expect(html).toContain('ISO 160')
    expect(html).toContain('标签')
    expect(html).toContain('日本')
  })

  test('rounds small megapixel counts instead of showing zero', () => {
    const html = renderPhotoInfo(null, {
      id: 0,
      name: 'studio-test-editorial-v2-01.jpg',
      width: 1200,
      height: 800,
      size_bytes: 393479,
      format: 'JPG',
    })

    expect(html).toContain('1 MP')
    expect(html).not.toContain('0 MP')
  })
})

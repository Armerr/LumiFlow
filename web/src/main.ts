import { router } from './shared/router'
import { createFanPage } from './pages/fan'
import { createGridPage } from './pages/grid'
import { createDetailPage } from './pages/detail'

const app = document.getElementById('app')!

router.start(app, async (route) => {
  switch (route.page) {
    case 'fan':
      return createFanPage()
    case 'album':
      return createGridPage({ name: route.name })
    case 'detail':
      return createDetailPage({ album: route.album, photoId: route.photoId })
  }
})

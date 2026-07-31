import { router } from './shared/router'
import { createFanPage } from './pages/fan'
import { createGridPage } from './pages/grid'
import { createDetailPage } from './pages/detail'

const app = document.getElementById('app')!

async function startApp() {
  const status = await fetch('/api/auth/status').then((response) => response.json()) as { enabled: boolean, authenticated: boolean }
  if (status.enabled && !status.authenticated) {
    app.innerHTML = `<main style="min-height:100vh;display:grid;place-items:center;padding:24px"><form id="login-form" style="width:min(360px,100%);display:grid;gap:14px"><h1>输入访问密码</h1><input id="login-password" type="password" autocomplete="current-password" required autofocus><button type="submit">进入相册</button><p id="login-error" hidden>密码错误</p></form></main>`
    document.getElementById('login-form')?.addEventListener('submit', async (event) => {
      event.preventDefault()
      const password = (document.getElementById('login-password') as HTMLInputElement).value
      const response = await fetch('/api/auth/login', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ password }) })
      if (response.ok) window.location.reload()
      else document.getElementById('login-error')?.removeAttribute('hidden')
    })
    return
  }
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
}

void startApp()

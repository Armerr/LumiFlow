import { router } from './shared/router'
import { createFanPage } from './pages/fan'
import { createGridPage } from './pages/grid'
import { createDetailPage } from './pages/detail'
import './login.scss'

const app = document.getElementById('app')!

async function startApp() {
  const status = await fetch('/api/auth/status').then((response) => response.json()) as { enabled: boolean, authenticated: boolean }
  if (status.enabled && !status.authenticated) {
    app.innerHTML = `<main class="login-page"><form id="login-form" class="login-panel"><p class="login-kicker">LUMIFLOW</p><h1>照片库访问</h1><p class="login-copy">输入密码后继续浏览你的照片。</p><label for="login-password">访问密码</label><input id="login-password" type="password" autocomplete="current-password" autocapitalize="none" required autofocus><button id="login-submit" type="submit">进入相册</button><p id="login-error" class="login-error" role="alert" hidden></p></form></main>`
    document.getElementById('login-form')?.addEventListener('submit', async (event) => {
      event.preventDefault()
      const password = (document.getElementById('login-password') as HTMLInputElement).value
      const submit = document.getElementById('login-submit') as HTMLButtonElement
      const error = document.getElementById('login-error')!
      submit.disabled = true
      error.hidden = true
      try {
        const response = await fetch('/api/auth/login', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ password }) })
        if (response.ok) {
          window.location.reload()
          return
        }
        error.textContent = '密码错误，请重试。'
        error.hidden = false
      } catch {
        error.textContent = '无法连接到照片库，请稍后重试。'
        error.hidden = false
      } finally {
        submit.disabled = false
      }
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

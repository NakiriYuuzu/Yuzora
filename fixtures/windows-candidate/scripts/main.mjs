import { label } from './status.mjs'
const status = document.getElementById('module-status')
if (status) status.textContent = label
let count = 0
document.getElementById('counter')?.addEventListener('click', event => { event.currentTarget.textContent = `點擊次數：${++count}` })
const host = document.getElementById('shadow-host')
if (host) {
  const shadow = host.attachShadow({ mode: 'open' })
  const button = document.createElement('button')
  button.id = 'shadow-button'
  button.textContent = 'Open shadow DOM 元素'
  shadow.append(button)
}
const large = document.getElementById('large-element')
if (large) large.textContent = '大元素測試 ABC 123。'.repeat(10000)
document.getElementById('boundary')?.addEventListener('click', async () => {
  const result = document.getElementById('boundary-result')
  try {
    const response = await fetch('/%2e%2e%2foutside.txt')
    result.textContent = response.ok ? 'FAIL：越界資源可讀取' : `PASS：越界請求被拒絕（${response.status}）`
  } catch { result.textContent = 'PASS：越界請求被拒絕' }
})

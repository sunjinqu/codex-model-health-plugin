import { createRequire } from 'node:module'
import { mkdirSync, writeFileSync } from 'node:fs'
import assert from 'node:assert/strict'
import { fileURLToPath } from 'node:url'

const require=createRequire(import.meta.url)
const {chromium}=require(process.env.PLAYWRIGHT_PATH || 'playwright')
const root=fileURLToPath(new URL('../',import.meta.url))
mkdirSync(`${root}/review/screenshots`,{recursive:true})
const browser=await chromium.launch({headless:true,executablePath:process.env.CHROME_BINARY,args:['--disable-gpu']})
const page=await browser.newPage({viewport:{width:1440,height:1050},deviceScaleFactor:1})
const errors=[]
page.on('pageerror',e=>errors.push(e.message))
const base=process.env.PREVIEW_URL || 'http://127.0.0.1:4175/'
async function shot(name){await page.waitForTimeout(300);await page.screenshot({path:`${root}/review/screenshots/${name}.png`,fullPage:true,animations:'disabled'})}
async function select(label, option){const trigger=page.getByRole('combobox',{name:label,exact:true});await trigger.click();const controls=await trigger.getAttribute('aria-controls');await page.locator(`[id="${controls}"]`).getByRole('option',{name:option,exact:true}).click()}
const rows=()=>page.locator('tbody tr:has(.account-cell)')
const runButton=()=>page.locator('.run-button')
async function open(query=''){await page.goto(`${base}?preview=1${query}`);await page.getByText('user1@example.com',{exact:true}).waitFor()}
async function waitIdle(){await page.getByText('本轮结束',{exact:false}).waitFor({timeout:20000})}
function noOverflow(label){return page.evaluate(()=>document.documentElement.scrollWidth > innerWidth+1).then(v=>assert.equal(v,false,`${label} must not overflow horizontally`))}

try {
  await open()
  await page.getByText('gpt-6.1-sol(low)',{exact:true}).first().waitFor()
  assert.equal(await rows().count(),7,'disabled accounts are hidden by default')
  assert.match(await runButton().textContent(),/测试全部 \(7\)/)
  assert.match(await page.locator('.legend-scope').textContent(),/gpt-6\.1-sol\(low\)/)
  await shot('light-list')

  await page.getByText('查看题目',{exact:true}).click()
  assert.match(await page.getByRole('textbox',{name:'糖果题题目'}).inputValue(),/五角星形/)
  await page.getByText('查看题目',{exact:true}).click()

  await page.getByRole('switch',{name:'仅启用'}).click({force:true})
  assert.equal(await rows().count(),8)
  const disabledRow=rows().filter({hasText:'user6@example.com'})
  await disabledRow.getByText('已停用',{exact:true}).waitFor()
  assert.equal(await disabledRow.getByRole('button',{name:'测试 user6@example.com'}).isDisabled(),true)
  await page.getByRole('switch',{name:'仅启用'}).click({force:true})

  await page.getByRole('textbox',{name:'搜索账号'}).fill('user3')
  assert.equal(await rows().count(),1)
  await page.getByRole('textbox',{name:'搜索账号'}).fill('')
  await select('订阅类型','Plus')
  assert.equal(await rows().count(),2)
  await select('订阅类型','全部订阅类型')

  const before=await page.locator('.rate b').first().textContent()
  await select('推理强度','high')
  assert.match(await page.locator('.legend-scope').textContent(),/gpt-6\.1-sol\(high\)/)
  assert.equal(await page.locator('.rate b').count(),0,'no answers were graded at high effort yet')
  await select('推理强度','low')
  assert.equal(await page.locator('.rate b').first().textContent(),before)

  await page.getByRole('checkbox',{name:'选择 user1@example.com'}).check({force:true})
  await page.getByRole('checkbox',{name:'选择 user2@example.com'}).check({force:true})
  assert.match(await runButton().textContent(),/测试所选 \(2\)/)
  const marksBefore=await rows().filter({hasText:'user2@example.com'}).locator('.mark-hit').count()
  await page.locator('.field-narrow').first().getByRole('button',{name:'增加每账号次数'}).click()
  await runButton().click()
  await page.getByRole('button',{name:'停止',exact:true}).waitFor()
  await shot('running')
  await waitIdle()
  assert.match(await page.locator('.batch-facts').textContent(),/4\/4/)
  assert.equal(await rows().filter({hasText:'user2@example.com'}).locator('.mark-hit').count(),Math.min(20,marksBefore+2))
  assert.equal(await page.getByText('排队中',{exact:true}).count(),0)
  assert.equal(await page.getByRole('button',{name:'测试 user3@example.com'}).isDisabled(),false)

  await rows().filter({hasText:'user1@example.com'}).locator('.mark-hit').first().click()
  await page.locator('.history-item.focus .answer-body').waitFor()
  await page.locator('.history-item.focus .answer-body').getByText(/preview-request-/).waitFor()
  await page.getByRole('button',{name:'收起 user1@example.com 的测试记录'}).waitFor()
  await shot('history')

  await page.getByRole('button',{name:'清空所选 2 个账号的测试记录'}).click()
  await page.getByRole('alertdialog').getByRole('button',{name:'清空',exact:true}).click()
  await rows().filter({hasText:'user2@example.com'}).getByText('未测试',{exact:true}).waitFor()
  await rows().filter({hasText:'user3@example.com'}).locator('.mark-hit').first().waitFor()

  await page.getByRole('checkbox',{name:'选择全部可测试账号'}).check({force:true})
  assert.match(await runButton().textContent(),/测试所选 \(7\)/)
  await page.getByRole('checkbox',{name:'选择全部可测试账号'}).uncheck({force:true})
  assert.match(await runButton().textContent(),/测试全部 \(7\)/)
  await page.getByRole('button',{name:'测试 user5@example.com'}).click()
  await waitIdle()
  assert.equal(await rows().filter({hasText:'user5@example.com'}).locator('.mark-hit').count(),2,'per-account runs setting applies to single tests')

  await open('&theme=dark')
  await runButton().click()
  await page.getByText('排队中',{exact:true}).first().waitFor()
  await shot('dark-running')
  await page.getByRole('button',{name:'停止',exact:true}).click()
  await waitIdle()
  assert.match(await page.locator('.batch-facts').textContent(),/跳过/)
  assert.match(await page.locator('.batch-facts').textContent(),/本轮结束 7\/7/,'stopped runs are counted as skipped')
  await page.setViewportSize({width:390,height:844})
  await shot('mobile')
  await noOverflow('mobile list')
  await page.setViewportSize({width:1440,height:1050})

  await page.goto(`${base}?preview=1&scenario=many`)
  await page.getByText('账号',{exact:false}).first().waitFor()
  await page.getByText('user60@example.com',{exact:true}).waitFor()
  await page.goto(`${base}?preview=1&scenario=empty`)
  await page.getByText('没有符合条件的账号',{exact:false}).waitFor()
  assert.equal(await runButton().isDisabled(),true)
  await shot('empty')
  await page.goto(`${base}?preview=1&scenario=error`)
  await page.getByRole('alert').getByText('模拟：宿主账号目录读取失败，请重试',{exact:true}).waitFor()
  await shot('error')
  await page.goto(`${base}?preview=1&scenario=loading`)
  assert.equal(await page.getByRole('button',{name:'刷新账号和记录'}).isDisabled(),true)
  await shot('loading')
  assert.deepEqual(errors,[])
  const result='BROWSER: list/prompt/enabled-filter/search/plan-filter/effort-scope/selection/batch-run/runs-per-account/queued/history/mark-focus/clear/single-test/stop/dark/mobile/many/empty/error/loading PASS; console_errors=0; page_overflow=0\n'
  writeFileSync(`${root}/review/BROWSER-QA.txt`,result)
  console.log(result.trim())
} finally {await browser.close()}

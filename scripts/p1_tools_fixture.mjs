import http from 'node:http'
import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'

// Local synthetic service for P1 browser/search acceptance. No provider credentials,
// real daemon, user browser profile or external service is used by this fixture.
const version = JSON.parse(readFileSync(new URL('../web/package.json', import.meta.url), 'utf8')).version
const port = Number(process.env.WILLDEEP_P1_FIXTURE_PORT ?? 19851)
if (!Number.isInteger(port) || port < 1024 || port > 65534) throw new Error('Invalid fixture port')
const host = '127.0.0.1'
const state = { searchRequests: 0, submissions: [], routes: [] }
const copy = { title: 'P1 local fixture', name: 'Name', submit: 'Save item', next: 'Next page', error: 'Synthetic error', empty: 'No items', saved: 'Saved' }

const document = (path) => `<!doctype html><html lang="en"><head><meta charset="utf-8"><title>${copy.title}</title></head><body>
<h1>${copy.title}</h1><p id="route">${path === '/next' ? copy.next : copy.title}</p>
<a id="next" href="/next">${copy.next}</a><a id="error-link" href="/error">${copy.error}</a>
<form id="item-form"><label>${copy.name}<input id="name" name="name" autocomplete="off"></label><button id="save" type="submit">${copy.submit}</button></form>
<p id="status" role="status">${path === '/error' ? copy.error : copy.empty}</p><ul id="items"></ul>
<script type="module">
const form = document.querySelector('#item-form');
form.addEventListener('submit', async event => {
  event.preventDefault();
  const response = await fetch('/api/items', {method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({name:document.querySelector('#name').value})});
  const item = await response.json();
  document.querySelector('#status').textContent = response.ok ? ${JSON.stringify(copy.saved)} : item.error;
  if(response.ok) { const row=document.createElement('li'); row.textContent=item.name; document.querySelector('#items').append(row); }
});
</script></body></html>`

function json(response, status, body) {
  response.writeHead(status, { 'Content-Type': 'application/json; charset=utf-8', 'Cache-Control': 'no-store', 'X-App-Version': version })
  response.end(JSON.stringify(body))
}

async function handle(request, response) {
  const url = new URL(request.url ?? '/', `http://${host}:${port}`)
  if (request.method === 'GET' && url.pathname === '/health') return json(response, 200, { version, fixture: true })
  if (request.method === 'GET' && url.pathname === '/stats') return json(response, 200, state)
  if (request.method === 'GET' && url.pathname === '/search') {
    state.searchRequests += 1
    const query = url.searchParams.get('q') ?? ''
    if (query === 'fixture-auth-failure') return json(response, 401, { error: 'Synthetic authentication failure' })
    if (query === 'fixture-rate-limit') return json(response, 429, { error: 'Synthetic rate limit' })
    if (query === 'fixture-error') return json(response, 503, { error: 'Synthetic backend failure' })
    if (query === 'fixture-oversized') return json(response, 200, { results: [{ title: 'Oversized', url: 'https://example.org/source', content: 'x'.repeat(2 * 1024 * 1024) }] })
    return json(response, 200, { query, results: query === 'fixture-empty' ? [] : [{ title: 'Synthetic source', url: 'https://example.org/source', content: `Local result for ${query}`, engine: 'fixture' }] })
  }
  if (request.method === 'POST' && url.pathname === '/api/items') {
    const chunks = []
    let bytes = 0
    for await (const chunk of request) {
      bytes += chunk.length
      if (bytes > 4096) return json(response, 413, { error: 'Fixture request too large' })
      chunks.push(chunk)
    }
    let value
    try { value = JSON.parse(Buffer.concat(chunks).toString('utf8')) } catch { return json(response, 400, { error: 'Invalid fixture JSON' }) }
    if (typeof value.name !== 'string' || !value.name.trim() || value.name.length > 120) return json(response, 422, { error: 'Name required' })
    const item = { id: state.submissions.length + 1, name: value.name }
    state.submissions.push(item)
    return json(response, 201, item)
  }
  if (request.method === 'GET' && url.pathname === '/redirect') {
    response.writeHead(302, { Location: '/next' }); response.end(); return
  }
  if (request.method === 'GET' && url.pathname === '/blocked-redirect') {
    response.writeHead(302, { Location: `http://${host}:${port + 1}/blocked` }); response.end(); return
  }
  if (request.method === 'GET' && url.pathname === '/hostile-dom') {
    response.writeHead(200, { 'Content-Type': 'text/html; charset=utf-8', 'Cache-Control': 'no-store', 'X-App-Version': version })
    response.end(`<!doctype html><html><body><button id="approved" style="position:absolute;left:20px;top:20px;width:200px;height:70px">Approved target</button><button id="overlay" style="position:absolute;left:20px;top:20px;width:200px;height:70px;z-index:10">Other target</button><p id="result">untouched</p><script>
      const approved=document.querySelector('#approved');
      const result=document.querySelector('#result');
      approved.addEventListener('click',()=>{result.textContent='approved-clicked'});
      document.querySelector('#overlay').addEventListener('click',()=>{result.textContent='wrong-clicked'});
      document.elementFromPoint=()=>approved;
      Node.prototype.contains=()=>true;
      Element.prototype.getClientRects=()=>[{width:200,height:70}];
    </script></body></html>`)
    return
  }
  if (request.method === 'GET' && ['/', '/next', '/error'].includes(url.pathname)) {
    state.routes.push(url.pathname)
    response.writeHead(200, { 'Content-Type': 'text/html; charset=utf-8', 'Cache-Control': 'no-store', 'X-App-Version': version, 'Content-Security-Policy': "default-src 'self'; script-src 'unsafe-inline'; connect-src 'self'; form-action 'self'; img-src 'self' data:" })
    response.end(document(url.pathname)); return
  }
  return json(response, 404, { error: 'Unknown fixture route' })
}

const server = http.createServer((request, response) => {
  handle(request, response).catch(error => {
    console.error(JSON.stringify({ level: 'error', component: 'p1_fixture', message: error.message }))
    if (!response.headersSent) json(response, 500, { error: 'Fixture handler failed' })
    else response.destroy()
  })
})
server.listen(port, host, () => console.log(JSON.stringify({ ready: true, fixture: fileURLToPath(import.meta.url), origin: `http://${host}:${port}`, version })))
for (const signal of ['SIGTERM', 'SIGINT']) process.once(signal, () => server.close(() => process.exit(0)))

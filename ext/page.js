async function page(cmd, a) {
  const S = (self.__min ||= { map: new Map(), next: 1 })
  const H = innerHeight
  const SKIP = new Set(["script", "style", "noscript", "template", "head", "svg", "canvas", "video", "audio", "img", "picture", "iframe", "object", "embed", "br", "hr"])
  const BLOCK = new Set(["p", "div", "section", "article", "main", "header", "footer", "nav", "aside", "form", "fieldset", "ul", "ol", "li", "dl", "dt", "dd", "table", "tr", "h1", "h2", "h3", "h4", "h5", "h6", "pre", "blockquote", "figure", "figcaption", "details", "dialog"])
  const ROLES = new Set(["button", "link", "checkbox", "radio", "switch", "tab", "menuitem", "menuitemcheckbox", "menuitemradio", "option", "combobox", "textbox", "searchbox", "slider", "spinbutton", "treeitem"])
  const TEXTY = (e) => e.localName === "textarea" || (e.localName === "input" && !["submit", "button", "reset", "image", "file", "range", "color", "checkbox", "radio"].includes(e.type))
  const clean = (s) => (s || "").replace(/\s+/g, " ").trim()
  const kids = (n) => n.shadowRoot ? n.shadowRoot.childNodes : n.localName === "slot" ? n.assignedNodes({ flatten: true }) : n.childNodes
  const control = (e) => {
    const t = e.localName
    return (t === "a" && e.hasAttribute("href")) || t === "button" || t === "select" || t === "textarea" || (t === "input" && e.type !== "hidden") || t === "summary" || ROLES.has(e.getAttribute("role")) || (e.isContentEditable && !e.parentElement?.isContentEditable)
  }
  const shown = (e) => e.checkVisibility({ checkOpacity: true, checkVisibilityCSS: true })

  function label(e) {
    const by = e.getAttribute("aria-labelledby"), t = e.localName, field = t === "input" || t === "select" || t === "textarea"
    const l = clean(
      e.getAttribute("aria-label") ||
      (by && by.split(/\s+/).map((i) => document.getElementById(i)?.textContent || "").join(" ")) ||
      (e.labels?.length && (e.textContent ? e.labels[0].textContent.replace(e.textContent, "") : e.labels[0].textContent)) ||
      e.placeholder ||
      (t === "input" && ["submit", "button", "reset"].includes(e.type) && e.value) ||
      (!field && (e.textContent.length < 300 ? e.innerText : e.textContent.slice(0, 300))) ||
      e.title || e.getAttribute("alt") || e.querySelector?.("img[alt]")?.alt || e.querySelector?.("svg title")?.textContent || e.name || ""
    )
    return l.length > 100 ? l.slice(0, 100).replace(/\s+\S*$/, "") + "…" : l
  }
  function role(e) {
    const r = e.getAttribute("role"), t = e.localName
    if (r && ROLES.has(r)) return r
    if (t === "a") return "link"
    if (t === "select") return "select"
    if (t === "textarea") return "textbox"
    if (t === "input") return { checkbox: "checkbox", radio: "radio", submit: "button", button: "button", reset: "button", image: "button", range: "slider", text: "textbox" }[e.type] || e.type
    return e.isContentEditable ? "editor" : "button"
  }
  function state(e) {
    const s = [], t = e.localName, ch = e.getAttribute("aria-checked"), ex = e.getAttribute("aria-expanded")
    if (t === "select") {
      const o = [...e.options].map((x) => clean(x.text))
      s.push(`= "${clean(e.selectedOptions[0]?.text)}"`, `(${o.slice(0, 12).join(" | ")}${o.length > 12 ? " | …" : ""})`)
    } else if (e.type === "checkbox" || e.type === "radio" || ch) s.push(e.checked || ch === "true" ? "on" : "off")
    else if (e.type === "password") s.push(e.value ? "(filled)" : "(empty)")
    else if (TEXTY(e)) s.push(e.value ? `= "${clean(e.value).slice(0, 60)}"` : "(empty)")
    else if (e.isContentEditable) { const v = clean(e.innerText); s.push(v ? `= "${v.slice(0, 60)}"` : "(empty)") }
    if (ex) s.push(ex === "true" ? "open" : "closed")
    if (e.disabled || e.getAttribute("aria-disabled") === "true") s.push("disabled")
    return s.length ? " " + s.join(" ") : ""
  }
  function tag(e) {
    const id = S.next++
    S.map.set(id, e)
    const l = label(e)
    const hint = l ? ` "${l}"` : ` (${(e.id || e.getAttribute("class") || "no label").split(/\s+/)[0].slice(0, 24)})`
    return `⟦${id}⟧ ${role(e)}${hint}${state(e)}`
  }

  function see(budget) {
    S.map = new Map()
    S.next = 1
    const lo = document.documentElement.scrollHeight > 4 * H ? -0.5 * H : -Infinity
    const modal = [...document.querySelectorAll("dialog, [aria-modal=true]")].find((d) => (d.localName !== "dialog" || d.matches(":modal")) && shown(d))
    const buf = ["", ""]
    const walk = (n, far) => {
      for (const c of kids(n)) {
        if (c.nodeType === 3) { const v = c.nodeValue; buf[far] += v.trim() ? (v.length > 500 ? v.slice(0, 500) + "…" : v) : v ? " " : ""; continue }
        if (c.nodeType !== 1) continue
        const t = c.localName
        if (SKIP.has(t) || c.getAttribute("aria-hidden") === "true") continue
        if (!shown(c)) { if (getComputedStyle(c).display === "contents") walk(c, far); continue }
        const r = c.getBoundingClientRect()
        if (r.height > 0 && r.bottom < lo) continue
        const f = far || (r.height > 0 && r.top > 1.5 * H) ? 1 : 0
        if (buf[f].length > budget) continue
        if (control(c)) { buf[f] += " " + tag(c) + " "; continue }
        const b = BLOCK.has(t)
        if (b) buf[f] += "\n"
        if (/^h[1-6]$/.test(t)) buf[f] += "#".repeat(+t[1]) + " "
        walk(c, f)
        if (b) buf[f] += "\n"
      }
    }
    walk(modal || document.body || document.documentElement, 0)
    const tidy = (x) => x.replace(/[ \t\u00a0]+/g, " ").replace(/ ?\n ?/g, "\n").replace(/^#+ ?$/gm, "").replace(/\n{2,}/g, "\n").trim()
    const near = tidy(buf[0]), far = tidy(buf[1])
    let text = near.length > budget ? near.slice(0, budget) + "\n…" : near
    if (far && text.length < budget) text += "\n" + (far.length > budget - text.length ? far.slice(0, budget - text.length) + "\n…" : far)
    const total = document.documentElement.scrollHeight
    const pos = total > H * 1.2 ? `, ${Math.round((scrollY / Math.max(1, total - H)) * 100)}% down` : ""
    return { t: document.title, u: location.href, w: innerWidth, h: H, x: text, pos, modal: !!modal, more: text.endsWith("…") }
  }


  function find(q) {
    const words = q.toLowerCase().split(/\s+/).filter((w) => w.length > 1)
    const has = (x) => words.some((w) => x.toLowerCase().includes(w))
    const hits = [], texts = []
    const visit = (n) => {
      for (const c of kids(n)) {
        if (hits.length >= 12 && texts.length >= 6) return
        if (c.nodeType === 3) {
          const v = clean(c.nodeValue)
          if (v.length > 2 && texts.length < 6 && has(v)) {
            let e = c.parentElement
            while (e && e.parentElement && clean(e.innerText).length < 80) e = e.parentElement
            texts.push("text: " + clean(e?.innerText || v).slice(0, 300))
          }
          continue
        }
        if (c.nodeType !== 1 || SKIP.has(c.localName)) continue
        if (!shown(c)) { if (getComputedStyle(c).display === "contents") visit(c); continue }
        if (control(c)) { if (hits.length < 12 && has(label(c) + " " + (c.value || ""))) hits.push(tag(c)); continue }
        visit(c)
      }
    }
    visit(document.body || document.documentElement)
    return [...new Set(texts), ...hits]
  }

  function click(e) {
    const link = e.closest("a[href]")
    if (link?.target === "_blank" && /^https?:/.test(link.href)) return { open: link.href }
    e.scrollIntoView({ block: "center", inline: "center", behavior: "instant" })
    const r = e.getBoundingClientRect(), x = r.left + r.width / 2, y = r.top + r.height / 2
    let top = document.elementFromPoint(x, y)
    while (top?.shadowRoot) { const d = top.shadowRoot.elementFromPoint(x, y); if (!d || d === top) break; top = d }
    const inside = top && (top === e || e.contains(top) || e.shadowRoot?.contains(top))
    const t = inside ? top : e
    const base = { bubbles: true, cancelable: true, composed: true, clientX: x, clientY: y, view: window }
    const pe = (type, o) => t.dispatchEvent(new PointerEvent(type, { ...base, pointerId: 1, pointerType: "mouse", isPrimary: true, ...o }))
    const me = (type, o) => t.dispatchEvent(new MouseEvent(type, { ...base, ...o }))
    pe("pointerover"); me("mouseover"); pe("pointerdown", { button: 0, buttons: 1 }); me("mousedown", { button: 0, buttons: 1 })
    ;(t.closest?.("a,button,input,select,textarea,summary,[tabindex],[contenteditable]") || e).focus?.({ preventScroll: true })
    pe("pointerup", { button: 0, buttons: 0 }); me("mouseup", { button: 0, buttons: 0 }); me("click", { button: 0, detail: 1 })
    return { ok: inside || !top ? "" : ` (it was covered by ${top.localName}${top.id ? "#" + top.id : ""})` }
  }

  function enter(e) {
    const o = { key: "Enter", code: "Enter", keyCode: 13, which: 13, bubbles: true, cancelable: true, composed: true }
    const go = e.dispatchEvent(new KeyboardEvent("keydown", o))
    e.dispatchEvent(new KeyboardEvent("keypress", o))
    e.dispatchEvent(new KeyboardEvent("keyup", o))
    if (go && e.form && e.localName !== "textarea") e.form.requestSubmit ? e.form.requestSubmit() : e.form.submit()
  }

  function type(e, text, submit) {
    e.scrollIntoView({ block: "center", behavior: "instant" })
    e.focus({ preventScroll: true })
    if (e.localName === "select") {
      const o = [...e.options].find((x) => clean(x.text).toLowerCase() === text.toLowerCase()) || [...e.options].find((x) => x.text.toLowerCase().includes(text.toLowerCase()))
      if (!o) return { err: `no option "${text}"` }
      e.value = o.value
    } else if (e.isContentEditable) {
      getSelection().selectAllChildren(e)
      if (!document.execCommand("insertText", false, text)) e.textContent = text
    } else if (TEXTY(e)) {
      const set = Object.getOwnPropertyDescriptor(Object.getPrototypeOf(e), "value")?.set
      set ? set.call(e, text) : (e.value = text)
      e.dispatchEvent(new InputEvent("input", { bubbles: true, composed: true, inputType: "insertText", data: text }))
    } else return { err: `that is a ${role(e)}, not a text field. use click for it.` }
    e.dispatchEvent(new Event("change", { bubbles: true }))
    if (submit) enter(e)
    return {}
  }

  function press(key) {
    const e = document.activeElement || document.body
    if (key === "Enter") return enter(e)
    if (key === "Tab") {
      const f = [...document.querySelectorAll("a[href],button,input,select,textarea,[tabindex]:not([tabindex='-1'])")].filter((x) => !x.disabled && shown(x))
      return f[(f.indexOf(e) + 1) % f.length]?.focus()
    }
    const o = { key, code: key, bubbles: true, cancelable: true, composed: true }
    e.dispatchEvent(new KeyboardEvent("keydown", o))
    e.dispatchEvent(new KeyboardEvent("keyup", o))
  }

  function scroll(up) {
    const d = (up ? -0.85 : 0.85) * H, y = scrollY
    scrollBy({ top: d, behavior: "instant" })
    if (scrollY !== y) return
    let s = document.elementFromPoint(innerWidth / 2, H / 2)
    while (s && !(s.scrollHeight > s.clientHeight + 4 && /auto|scroll/.test(getComputedStyle(s).overflowY))) s = s.parentElement
    s?.scrollBy({ top: d, behavior: "instant" })
  }

  const quiet = (ms) => new Promise((done) => {
    let t = setTimeout(end, 50)
    const o = new MutationObserver(() => { clearTimeout(t); t = setTimeout(end, 50) })
    const cap = setTimeout(end, ms)
    o.observe(document.documentElement, { subtree: true, childList: true, characterData: true })
    function end() { o.disconnect(); clearTimeout(t); clearTimeout(cap); done() }
  })

  if (cmd === "see") { if (a.wait) await quiet(500); return see(a.budget) }
  if (cmd === "find") return find(a.q)
  if (cmd === "read") {
    const x = ((document.body || document.documentElement).innerText || "").replace(/[ \t\u00a0]+/g, " ").replace(/\n\s*\n+/g, "\n").trim()
    return { t: document.title, u: location.href, x: x.slice(a.from, a.from + 4000), n: x.length }
  }
  if (cmd === "scroll") return scroll(a.up)
  if (cmd === "press") return press(a.key)
  const e = S.map.get(a.id)
  if (!e?.isConnected) return { err: "that element is gone. look at the new view." }
  return cmd === "click" ? click(e) : type(e, String(a.text ?? ""), a.enter)
}

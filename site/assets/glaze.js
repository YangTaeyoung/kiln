/* Fired-glaze surfaces. One shared WebGL context paints every [data-glaze] element into its own 2D canvas. */
(function () {
  var TYPES = { tenmoku: 0, celadon: 1, shino: 2, kaki: 3 };
  var VS = "attribute vec2 p;void main(){gl_Position=vec4(p,0.,1.);}";
  var FS = [
    "precision highp float;",
    "uniform vec2 r;uniform vec2 m;uniform int g;uniform float seed;",
    "float h1(vec2 p){vec3 p3=fract(vec3(p.xyx)*.1031+seed*.0137);p3+=dot(p3,p3.yzx+33.33);return fract((p3.x+p3.y)*p3.z);}",
    "float n2(vec2 p){vec2 i=floor(p),f=fract(p);f=f*f*(3.-2.*f);return mix(mix(h1(i),h1(i+vec2(1,0)),f.x),mix(h1(i+vec2(0,1)),h1(i+vec2(1,1)),f.x),f.y);}",
    "float fbm(vec2 p){float v=0.,a=.5;for(int i=0;i<5;i++){v+=a*n2(p);p=p*2.03+vec2(3.1,1.7);a*=.5;}return v;}",
    "vec2 vor(vec2 p){vec2 i=floor(p),f=fract(p);float d1=8.,d2=8.;for(int y=-1;y<=1;y++)for(int x=-1;x<=1;x++){vec2 o=vec2(x,y);vec2 c=o+vec2(h1(i+o),h1(i+o+7.3))*.9-f;float d=dot(c,c);if(d<d1){d2=d1;d1=d;}else if(d<d2)d2=d;}return vec2(sqrt(d1),sqrt(d2)-sqrt(d1));}",
    "float surf(vec2 p){return fbm(p*1.1)*.75+fbm(p*2.6)*.06;}",
    "void main(){",
    " vec2 uv=gl_FragCoord.xy/r;vec2 asp=vec2(r.x/r.y,1.);vec2 p=uv*asp*1.6+seed;",
    " float edge=min(min(uv.x,1.-uv.x)*asp.x,min(uv.y,1.-uv.y));",
    " float thick=fbm(p*1.1+vec2(0.,-uv.y*.6));",
    " vec3 c;float gloss=1.;",
    " if(g==0){",
    "  float run=fbm(vec2(p.x*.8,p.y*.35));",
    "  float fur=fbm(vec2(p.x*70.+fbm(p*1.5)*6.,p.y*1.2));",
    "  float fur2=fbm(vec2(p.x*140.,p.y*2.+3.));",
    "  vec3 black=mix(vec3(.035,.025,.02),vec3(.075,.05,.035),thick);",
    "  vec3 gold=mix(vec3(.3,.17,.08),vec3(.52,.33,.16),fur2);",
    "  float streak=smoothstep(.6,.78,fur)*smoothstep(.25,1.,uv.y*.75+run*.5);",
    "  c=mix(black,gold,streak*.7);",
    "  c=mix(vec3(.36,.2,.1),c,smoothstep(.0,.022,edge));",
    " } else if(g==1){",
    "  vec3 thin=vec3(.74,.8,.73),deep=vec3(.44,.59,.52);",
    "  c=mix(thin,deep,smoothstep(.3,.75,thick));",
    "  vec2 v1=vor(p*3.2+fbm(p*1.6)*1.1);vec2 v2=vor(p*8.+9.);",
    "  float cr=1.-smoothstep(0.,.01,v1.y);float cr2=1.-smoothstep(0.,.007,v2.y);",
    "  c=mix(c,c*vec3(.72,.76,.72),cr*.6);c=mix(c,c*vec3(.88,.9,.88),cr2*.3);",
    "  c=mix(vec3(.6,.52,.42),c,smoothstep(.0,.018,edge));",
    " } else if(g==2){",
    "  vec3 white=mix(vec3(.95,.92,.87),vec3(.9,.86,.8),fbm(p*3.));",
    "  float fl=smoothstep(.45,.78,fbm(p*.9+vec2(1.,4.)));",
    "  vec3 flash=mix(vec3(.91,.62,.42),vec3(.8,.4,.22),fbm(p*2.5));",
    "  c=mix(white,flash,fl*.85);",
    "  float carbon=smoothstep(.78,.86,fbm(p*1.2+11.));c=mix(c,vec3(.4,.36,.33),carbon*.45);",
    "  c*=1.-step(.997,h1(floor(p*70.)))*.25;",
    "  gloss=.45;",
    " } else {",
    "  c=mix(vec3(.56,.19,.06),vec3(.71,.31,.11),fbm(p*2.2));",
    "  float iron=smoothstep(.62,.8,fbm(p*1.8+5.));c=mix(c,vec3(.26,.08,.04),iron*.55);",
    "  c=mix(vec3(.45,.3,.22),c,smoothstep(.0,.022,edge));",
    "  gloss=.85;",
    " }",
    " float e=.006;float s0=surf(p);vec3 N=normalize(vec3((s0-surf(p+vec2(e,0.)))*.22,(s0-surf(p+vec2(0.,e)))*.22,e*2.));",
    " vec2 hp=(uv-m)*asp+N.xy*1.1;vec2 d=abs(hp)-vec2(.26,.12);float box=1.-smoothstep(-.12,.32+.3*(1.-gloss),max(d.x,d.y));box*=box;",
    " c*=.9+.1*N.z;",
    " c+=vec3(1.,.98,.95)*box*(.16*gloss+.03);float sheen=smoothstep(.0,1.,1.-length(uv-vec2(m.x,m.y))*.9);c+=vec3(1.,.97,.92)*sheen*sheen*.05*gloss;",
    " c+=pow(1.-N.z,3.)*.08*gloss;",
    " c*=.84+.16*smoothstep(1.4,.3,length(uv-vec2(.5,.55)));",
    " gl_FragColor=vec4(c,1.);",
    "}"
  ].join("\n");

  var reduce = window.matchMedia && matchMedia("(prefers-reduced-motion: reduce)").matches;
  var glc = document.createElement("canvas");
  var gl = glc.getContext("webgl", { preserveDrawingBuffer: true, antialias: false });
  var tiles = [], mouse = { x: -1, y: -1 }, U = {};
  if (gl) {
    var sh = function (type, src) { var s = gl.createShader(type); gl.shaderSource(s, src); gl.compileShader(s); if (!gl.getShaderParameter(s, gl.COMPILE_STATUS)) console.error(gl.getShaderInfoLog(s)); return s; };
    var pr = gl.createProgram(); gl.attachShader(pr, sh(gl.VERTEX_SHADER, VS)); gl.attachShader(pr, sh(gl.FRAGMENT_SHADER, FS)); gl.linkProgram(pr); gl.useProgram(pr);
    var b = gl.createBuffer(); gl.bindBuffer(gl.ARRAY_BUFFER, b); gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 1, -1, -1, 1, 1, 1]), gl.STATIC_DRAW);
    var l = gl.getAttribLocation(pr, "p"); gl.enableVertexAttribArray(l); gl.vertexAttribPointer(l, 2, gl.FLOAT, false, 0, 0);
    ["r", "m", "g", "seed"].forEach(function (k) { U[k] = gl.getUniformLocation(pr, k); });
  }

  function mount(el) {
    if (el._glaze) return;
    var cv = document.createElement("canvas");
    cv.className = "glaze-canvas"; cv.setAttribute("aria-hidden", "true");
    el.insertBefore(cv, el.firstChild);
    var t = { el: el, cv: cv, ctx: cv.getContext("2d"), type: TYPES[el.dataset.glaze] || 0, seed: parseFloat(el.dataset.seed || "0"), lm: { x: .32, y: .7 }, visible: false, dirty: true };
    el._glaze = t; tiles.push(t); size(t);
    if ("IntersectionObserver" in window) new IntersectionObserver(function (es) { t.visible = es[0].isIntersecting; t.dirty = true; }, { rootMargin: "120px" }).observe(el);
    else t.visible = true;
  }
  function size(t) {
    var d = Math.min(window.devicePixelRatio || 1, 2), rc = t.el.getBoundingClientRect();
    var w = Math.max(1, Math.round(rc.width * d)), h = Math.max(1, Math.round(rc.height * d));
    if (t.cv.width !== w || t.cv.height !== h) { t.cv.width = w; t.cv.height = h; }
    t.dirty = true;
  }
  function paint(t) {
    if (!gl) { t.ctx.fillStyle = ["#1b120d", "#8fa79a", "#e9dfd2", "#8a3412"][t.type]; t.ctx.fillRect(0, 0, t.cv.width, t.cv.height); return; }
    var w = t.cv.width, h = t.cv.height;
    if (glc.width < w || glc.height < h) { glc.width = Math.max(glc.width, w); glc.height = Math.max(glc.height, h); }
    gl.viewport(0, 0, w, h);
    gl.uniform2f(U.r, w, h); gl.uniform2f(U.m, t.lm.x, t.lm.y); gl.uniform1i(U.g, t.type); gl.uniform1f(U.seed, t.seed);
    gl.drawArrays(gl.TRIANGLE_STRIP, 0, 4);
    t.ctx.drawImage(glc, 0, glc.height - h, w, h, 0, 0, w, h);
  }
  function frame() {
    for (var i = 0; i < tiles.length; i++) {
      var t = tiles[i]; if (!t.visible) continue;
      var rc = t.el.getBoundingClientRect(), tx = .32, ty = .7;
      if (mouse.x >= 0) { tx = (mouse.x - rc.left) / rc.width; ty = 1 - (mouse.y - rc.top) / rc.height; }
      tx = Math.max(-.6, Math.min(1.6, tx)); ty = Math.max(-.6, Math.min(1.6, ty));
      var k = reduce ? 1 : .08, nx = t.lm.x + (tx - t.lm.x) * k, ny = t.lm.y + (ty - t.lm.y) * k;
      var moved = Math.abs(nx - t.lm.x) > 2e-4 || Math.abs(ny - t.lm.y) > 2e-4;
      if (!t.dirty && !moved) continue;
      t.lm.x = nx; t.lm.y = ny; t.dirty = false; paint(t);
    }
    requestAnimationFrame(frame);
  }
  function init() {
    document.querySelectorAll("[data-glaze]").forEach(mount);
    var rt; window.addEventListener("resize", function () { clearTimeout(rt); rt = setTimeout(function () { tiles.forEach(size); }, 120); });
    if (!reduce) window.addEventListener("pointermove", function (e) { mouse.x = e.clientX; mouse.y = e.clientY; }, { passive: true });
    requestAnimationFrame(frame);
  }
  window.KilnGlaze = { refresh: function () { document.querySelectorAll("[data-glaze]").forEach(mount); tiles.forEach(size); } };
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", init); else init();
})();

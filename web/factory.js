let mapZoom=1,mapFitWidth=0,mapFitHeight=0;
function sizeFactoryMap(reset=false){
  const viewport=$('map-viewport');if(!viewport||!viewport.clientWidth)return;
  if(reset){mapZoom=1;mapFitWidth=viewport.clientWidth;mapFitHeight=viewport.clientHeight;}
  const map=$('factory-map');map.style.setProperty('--canvas-width',Math.round(mapFitWidth*mapZoom)+'px');map.style.setProperty('--canvas-height',Math.round(mapFitHeight*mapZoom)+'px');
  $('map-zoom-label').textContent=Math.round(mapZoom*100)+'%';
  if(reset){viewport.scrollLeft=0;viewport.scrollTop=0;}
}
function zoomFactory(factor){const v=$('map-viewport'),oldWidth=$('factory-map').offsetWidth,oldHeight=$('factory-map').offsetHeight;const cx=(v.scrollLeft+v.clientWidth/2)/oldWidth,cy=(v.scrollTop+v.clientHeight/2)/oldHeight;mapZoom=Math.max(.75,Math.min(3,mapZoom*factor));sizeFactoryMap();v.scrollLeft=cx*$('factory-map').offsetWidth-v.clientWidth/2;v.scrollTop=cy*$('factory-map').offsetHeight-v.clientHeight/2;}
function expandFactory(open){document.body.classList.toggle('map-expanded',open);$('map-expand').textContent=open?'退出大圖':'大圖檢視';$('map-expand').setAttribute('aria-expanded',String(open));requestAnimationFrame(()=>sizeFactoryMap(true));}
$('map-expand').onclick=()=>expandFactory(!document.body.classList.contains('map-expanded'));
$('map-order').hidden=role==='agv';$('map-order').onclick=()=>{expandFactory(false);if(window.matchMedia('(max-width:760px)').matches)setComposer(true);else{$('from').focus();$('order-panel').scrollIntoView({block:'nearest'});}};
$('map-zoom-in').onclick=()=>zoomFactory(1.25);$('map-zoom-out').onclick=()=>zoomFactory(.8);$('map-fit').onclick=()=>{$('map-focus').value='all';sizeFactoryMap(true);};
$('map-focus').onchange=()=>{const regions={machine:[48,18],charging:[93,51],standby:[25,92],idle:[70,92]};const p=regions[$('map-focus').value];if(!p){sizeFactoryMap(true);return;}mapZoom=1.65;sizeFactoryMap();const v=$('map-viewport'),m=$('factory-map');v.scrollLeft=m.offsetWidth*p[0]/100-v.clientWidth/2;v.scrollTop=m.offsetHeight*p[1]/100-v.clientHeight/2;};
document.addEventListener('keydown',e=>{if(e.key==='Escape')expandFactory(false);});
let mapDrag=null;
$('map-viewport').addEventListener('pointerdown',e=>{if(e.pointerType!=='mouse'||e.button!==0)return;const v=$('map-viewport');mapDrag={x:e.clientX,y:e.clientY,left:v.scrollLeft,top:v.scrollTop};v.setPointerCapture(e.pointerId);v.classList.add('dragging');e.preventDefault();});
$('map-viewport').addEventListener('pointermove',e=>{if(!mapDrag)return;const v=$('map-viewport');v.scrollLeft=mapDrag.left-(e.clientX-mapDrag.x);v.scrollTop=mapDrag.top-(e.clientY-mapDrag.y);});
function endMapDrag(){mapDrag=null;$('map-viewport').classList.remove('dragging');}
$('map-viewport').addEventListener('pointerup',endMapDrag);$('map-viewport').addEventListener('pointercancel',endMapDrag);
new ResizeObserver(()=>{if(mapZoom===1)sizeFactoryMap(true);}).observe($('map-viewport'));
requestAnimationFrame(()=>sizeFactoryMap(true));
function buildFactoryStations(graph){
  const map=$('factory-map');map.querySelectorAll('[data-node]').forEach(n=>n.remove());map.querySelectorAll('.junction-marker').forEach(n=>n.remove());
  const categories={machine:'生產機台',warehouse:'倉儲',charging:'充電停靠',standby:'待命區',idle:'空閒停車區',passing:'避讓點'};
  graph.nodes.filter(n=>n.kind==='station').forEach(n=>{
    names[n.id]=n.label||names[n.id]||n.id;coordinates[n.id]=[n.x,n.y];
    const card=document.createElement('div');card.className='station factory-station type-'+n.category+(n.x>90?' side-right':'');card.dataset.node=n.id;card.dataset.category=n.category;card.style.left=n.x+'%';card.style.top=n.y+'%';card.title=(n.label||n.id)+' · '+n.id+' · '+(categories[n.category]||'站點');
    const icon=document.createElement('span');icon.className='station-icon';icon.textContent=({charging:'ϟ',standby:'◎',idle:'P',warehouse:'▤',passing:'↔'}[n.category]||'▥');const label=document.createElement('b');label.textContent=n.label||n.id;const code=document.createElement('span');code.className='station-code';code.textContent=n.id;const status=document.createElement('small');status.textContent='空閒';card.append(icon,label,code,status);map.append(card);
  });
  for(const id of ['from','to']){const select=$(id),selected=select.value;select.replaceChildren();for(const category of ['machine','warehouse','charging','standby','idle','passing']){const group=document.createElement('optgroup');group.label=categories[category];graph.nodes.filter(n=>n.kind==='station'&&n.category===category).forEach(n=>{const option=document.createElement('option');option.value=n.id;option.textContent=n.label+' · '+n.id;group.append(option);});select.append(group);}select.value=graph.nodes.some(n=>n.id===selected)?selected:(id==='from'?'A':'B');}
  updateTaskKind();
  $('factory-count').textContent=graph.nodes.filter(n=>n.kind==='station').length+' 個站點 · '+graph.nodes.filter(n=>n.kind==='junction').length+' 個路徑點 · '+graph.edges.length+' 條道路';
}

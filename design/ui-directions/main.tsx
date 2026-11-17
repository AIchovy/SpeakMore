import { useState } from 'react'
import { createRoot } from 'react-dom/client'
import { MotionConfig, motion, AnimatePresence } from 'motion/react'
import { PiHouse, PiClockCounterClockwise, PiBookOpenText, PiSlidersHorizontal, PiCpu, PiGearSix, PiMicrophone, PiCheck, PiArrowUpRight, PiCaretDown, PiCommand, PiX } from 'react-icons/pi'
import { Button } from '../../src/components/ui/button'
import { Switch } from '../../src/components/ui/switch'
import { SpeakMoreMark } from '../../src/components/brand'
import '../../src/styles.css'
import './styles.css'

const directions = [
  { id: 'a', letter: 'A', name: '极简黑白', note: '清晰的字阶，安静的留白。', colors: ['#FFFFFF', '#F4F4F4', '#DEDEDE', '#717171', '#171717'], detail: '用黑白对比和精确间距建立层次，让内容成为视觉中心。', best: '适合你想要的简约、克制，也最能突出 D 标识。' },
  { id: 'b', letter: 'B', name: '雾蓝银灰', note: '轻盈、柔和，有一点温度。', colors: ['#F8FAFC', '#EBF0F6', '#D5DEE9', '#63758B', '#385575'], detail: '银灰侧栏与低饱和雾蓝强调色，保留充足空间和轻微的层次。', best: '适合全天打开的桌面工具，亲和而不显得花哨。' },
  { id: 'c', letter: 'C', name: '石墨深色', note: '沉静的深灰，细腻的明暗。', colors: ['#191A1C', '#222426', '#343639', '#A3A5A8', '#ECEDEF'], detail: '通过石墨灰的明暗区分区域，以柔白强调动作，避免大面积高亮色。', best: '适合偏爱深色界面与专业创作工具质感的使用习惯。' },
] as const
type Direction = typeof directions[number]

function App() {
  const [choice, setChoice] = useState<Direction>(directions[0])
  const [page, setPage] = useState<'overview' | 'settings'>('overview')
  const [recording, setRecording] = useState(false)
  const [sound, setSound] = useState(true)
  const [history, setHistory] = useState(false)
  const [model, setModel] = useState('Whisper Large v3 Turbo')
  const [mode, setMode] = useState('自然表达')
  const [detail, setDetail] = useState<number | null>(null)
  const [raw, setRaw] = useState(false)
  const results = ['新版本先把听写和自动输入做好，设置尽量简单，用户打开就能开始说话。', '明天下午两点和设计师确认新界面，重点看文字层级、录音状态和深色模式。']
  return <MotionConfig reducedMotion="user">
    <div className="review-shell">
      <header className="review-header"><div><span className="review-brand"><SpeakMoreMark />SpeakMore</span><h1>找到适合它的气质。</h1></div><p>三套视觉方向，同一组真实功能。<br/>点击切换方案，体验概览与设置。</p></header>
      <div className="direction-picker" aria-label="选择视觉方案">
        {directions.map(d => <button key={d.id} className={`direction-option ${choice.id === d.id ? 'chosen' : ''}`} aria-pressed={choice.id === d.id} onClick={() => setChoice(d)}>
          <div className="option-title"><span>{d.letter}</span><strong>{d.name}</strong>{choice.id === d.id && <PiCheck />}</div>
          <p>{d.note}</p><div className="swatches">{d.colors.map(c => <i key={c} style={{background:c}} />)}</div>
        </button>)}
      </div>
      <div className="preview-toolbar"><span><strong>{choice.letter} / {choice.name}</strong><span className="preview-hint">独立设计预览 · 示例内容与交互</span></span><div className="page-picker" aria-label="预览页面"><button aria-pressed={page==='overview'} onClick={()=>setPage('overview')}>概览</button><button aria-pressed={page==='settings'} onClick={()=>setPage('settings')}>设置</button></div></div>
      <div className={`app-window theme-${choice.id}`}>
        <aside className="app-sidebar">
          <div className="traffic"><i/><i/><i/></div>
          <div className="app-brand"><SpeakMoreMark/><span>SpeakMore</span></div>
          <nav aria-label="应用预览导航">
            <button onClick={()=>setPage('overview')} className={page==='overview'?'selected':''}><PiHouse/>概览</button>
            <span><PiClockCounterClockwise/>历史记录</span><span><PiBookOpenText/>词典</span><span><PiSlidersHorizontal/>写作模式</span><span><PiCpu/>语音模型</span>
          </nav>
          <div className="sidebar-bottom"><div className="model-ready"><span/>本地模型已就绪</div><button onClick={()=>setPage('settings')} className={page==='settings'?'selected':''}><PiGearSix/>设置</button></div>
        </aside>
        <main className="app-main">
          <div className="window-bar"><span>{page==='overview'?'概览':'设置'}</span><span className="window-shortcut"><PiCommand/> K</span></div>
          <AnimatePresence mode="wait">
            <motion.div key={page} className="workspace" initial={{opacity:0,y:3}} animate={{opacity:1,y:0}} exit={{opacity:0}} transition={{duration:.14}}>
              {page==='overview' ? <>
                <div className="page-heading"><div><p className="eyebrow">你的声音，自然成文</p><h2>想说的，现在就说。</h2></div><span className="day-label">周四，9 月 17 日</span></div>
                <section className={`dictation-stage ${recording?'is-recording':''}`} aria-label="听写预览">
                  <div className="stage-copy"><div className="stage-status"><span className="status-dot"/>{recording?'正在聆听':'准备好聆听'}</div><p>{recording?'慢慢说，停顿也没关系。':'说出想法，文字会跟上。'}</p><div className="shortcut-note"><kbd>fn</kbd><span>按一下说话，再按一下完成</span></div></div>
                  <div className="stage-action"><div className="sound-sculpture" aria-hidden>{[12,22,37,54,68,49,32,60,76,55,34,20,10].map((h,i)=><motion.i key={i} style={{height:h}} animate={recording?{scaleY:[.4,1,.55]}:{scaleY:1}} transition={recording?{duration:.55+i*.04,repeat:Infinity,repeatType:'reverse'}:{duration:.15}} />)}</div><Button className="record-button" onClick={()=>setRecording(!recording)}>{recording?<PiCheck/>:<PiMicrophone/>}{recording?'完成预览':'开始听写'}</Button></div>
                  <div className="stage-bottom"><span>{recording?'录音状态演示，未启用麦克风':'在其他应用使用 Fn，文字直接进入输入框。'}</span><span>本机识别 <b/> 最长 5 分钟</span></div>
                </section>
                <div className="session-options"><div><label htmlFor="demo-language">识别语言</label><div className="select-wrap"><select id="demo-language" defaultValue="自动检测"><option>自动检测</option><option>中文</option><option>English</option></select><PiCaretDown/></div></div><div><label htmlFor="demo-mode">文字整理</label><div className="select-wrap"><select id="demo-mode" value={mode} onChange={e=>setMode(e.target.value)}><option>自然表达</option><option>保留原文</option><option>邮件</option></select><PiCaretDown/></div></div><div><label htmlFor="demo-model">本地模型</label><div className="select-wrap"><select id="demo-model" value={model} onChange={e=>setModel(e.target.value)}><option>Whisper Large v3 Turbo</option><option>SenseVoice Small</option></select><PiCaretDown/></div></div></div>
                <section className="recent"><div className="section-heading"><h3>最近结果</h3><span>本次预览的示例文字</span></div>{results.map((r,i)=><button className="result-row" key={r} onClick={()=>{setDetail(i);setRaw(false)}}><span className="result-time">{i?'10:24':'11:42'}<small>{i?'24':'18'} 秒</small></span><span className="result-text">{r}<small>{i?'原文':'已整理'}<span>可查看完整文字</span></small></span><PiArrowUpRight/></button>)}</section>
              </> : <>
                <div className="page-heading"><div><p className="eyebrow">让工具适应你的习惯</p><h2>少一点设置，多一点表达。</h2></div></div>
                <div className="settings-tabs"><span className="active">通用</span><span>语音与模型</span><span>AI 服务</span><span>隐私与数据</span></div>
                <section className="settings-group"><h3>听写</h3><div className="setting-row"><div><h4>听写快捷键</h4><p>按一下开始，再按一下完成。</p></div><kbd className="large-key">fn</kbd></div><div className="setting-row"><div><h4>开始与结束音效</h4><p>轻声提示，让你知道听写何时开始和结束。</p></div><Switch aria-label="开始与结束音效" checked={sound} onCheckedChange={setSound}/></div><div className="setting-row"><div><h4>识别模型</h4><p>下载到这台 Mac，离线也能听写。</p></div><div className="select-wrap"><select aria-label="识别模型" value={model} onChange={e=>setModel(e.target.value)}><option>Whisper Large v3 Turbo</option><option>SenseVoice Small</option></select><PiCaretDown/></div></div></section>
                <section className="settings-group"><h3>记录</h3><div className="setting-row"><div><h4>保存历史记录</h4><p>在本机保留听写文字，方便之后查找。</p></div><Switch aria-label="保存历史记录" checked={history} onCheckedChange={setHistory}/></div></section>
              </>}
            </motion.div>
          </AnimatePresence>
          {detail!==null && <div className="detail-layer"><div className="detail-panel" role="dialog" aria-label="示例转写详情"><div className="detail-heading"><h3>转写详情</h3><Button variant="ghost" size="icon" aria-label="关闭详情" onClick={()=>setDetail(null)}><PiX/></Button></div><p className="detail-meta">今天 {detail?'10:24':'11:42'} · 示例结果</p>{detail===0?<div className="detail-tabs"><button aria-pressed={raw} onClick={()=>setRaw(true)}>原文</button><button aria-pressed={!raw} onClick={()=>setRaw(false)}>整理后</button></div>:<h4>原文</h4>}<p className="detail-text">{raw&&detail===0?'嗯，新版本的话，先把听写和自动输入做好吧，设置尽量简单一些，用户打开就能开始说话。':results[detail]}</p></div></div>}
        </main>
      </div>
      <footer className="direction-summary"><div><h2>{choice.name}</h2><p>{choice.detail}</p><p>{choice.best}</p></div><div className="palette-detail">{choice.colors.map(c=><span key={c}><i style={{background:c}}/>{c}</span>)}</div></footer>
      <p className="review-note">保留 D「双声」标识 · shadcn/ui 组件 · React Icons · Motion 交互 · 不修改当前应用</p>
    </div>
  </MotionConfig>
}
createRoot(document.getElementById('root')!).render(<App/> )

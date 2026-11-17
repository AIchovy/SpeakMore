import type { Translate } from '@/lib/errors'

// Only localize application messages, never meeting transcripts or user content.
const english: Record<string, string> = {
  '已保存的字幕不存在，可能已被删除。':
    'The saved captions are no longer available. They may have been deleted.',
  '请填写有效的具体日期，或留空待确认。':
    'Choose a valid calendar date, or leave it blank for confirmation.',
  '这条内容尚不是明确承诺，请在来源会议中检查内容性质。':
    'This is not an explicit commitment. Check the statement type in the source meeting.',
  '请先核对原文并确认负责人，再为未完成待办导出日历提醒。':
    'Check the sources and owner before exporting reminders for open tasks.',
  '便签请填写 1–1200 个字，重点标记不需要文字。':
    'Write a note of 1–1200 characters. Highlights do not need text.',
  '标记时间超出当前录音，请重新打开便签。':
    'This timestamp is ahead of the recording. Close and reopen the note.',
  '这场会议录音已结束，请打开会议查看。': 'This recording has ended. Open the meeting to view it.',
  '会议悬浮窗暂不可用，请在主窗口查看录音状态。':
    'The floating panel is unavailable. Check the recording status in the main window.',
  '录音已删除，仍可查看逐字稿。': 'The recording was deleted. The transcript is still available.',
  '本段录音还在保存，请稍后再试。': 'This segment is still being saved. Try again shortly.',
  '片段时间超出录音范围。': 'The segment timestamp is outside the recording.',
  '此片段没有可回听的录音，仍可查看逐字稿。':
    'There is no recording for this segment. The transcript is still available.',
  '本段录音文件缺失或损坏，仍可查看逐字稿。':
    'The audio file for this segment is missing or damaged. The transcript is still available.',
  '回听无法播放，请重新加载片段后再试。':
    'Could not play the audio. Reload the segment and try again.',
  '无法启动麦克风，请检查设备连接和系统声音输入设置。':
    'Could not start the microphone. Check the device connection and system sound input settings.',
  '麦克风启动超时，请检查设备连接后重试。':
    'Starting the microphone timed out. Check the device connection and try again.',
  'Zipformer 中英流式': 'Zipformer · Chinese and English streaming',
  'Zipformer 英语轻量流式': 'Zipformer · Lightweight English streaming',
  'Paraformer 中英流式': 'Paraformer · Chinese and English streaming',
  'MOSS 会议与说话人': 'MOSS · Meetings and speakers',
  'AI 回答为空或过长，请缩小问题范围。':
    'The AI response is empty or too long. Narrow your question and try again.',
  'AI 未返回完整回答，请重试或更换 AI 服务。':
    'The AI did not return a complete answer. Try again or choose another AI service.',
  'MOSS 处理超时，已保存的进度可继续；可改用轻量转写模型。':
    'MOSS timed out. Saved progress can be resumed, or you can choose a lighter transcription model.',
  'MOSS 本地转写失败，音频和已完成片段已保留。':
    'Local MOSS transcription failed. Audio and completed segments are kept.',
  'MOSS 本地运行环境不可用，请在设置 → 语音模型中重新安装运行环境。':
    'The local MOSS runtime is unavailable. Reinstall it in Settings → Voice models.',
  'MOSS 环境安装或检查失败，请重试。':
    'MOSS runtime installation or verification failed. Try again.',
  'MOSS 环境安装超时，请重试。': 'MOSS runtime installation timed out. Try again.',
  'MOSS 运行环境下载服务不可用，可重试。':
    'The MOSS runtime download service is unavailable. Try again.',
  'MOSS 运行环境下载连接失败，可重试。':
    'Could not connect to download the MOSS runtime. Try again.',
  'MOSS 运行环境目前支持 Apple 芯片的 Mac。':
    'The MOSS runtime currently supports Macs with Apple silicon.',
  'MOSS 进程中断，可继续已保存的进度。': 'MOSS was interrupted. You can resume saved progress.',
  'Notion 未完成请求，请核实发送状态后重试。':
    'Notion did not complete the request. Verify its status before retrying.',
  'Notion 连接密钥不可用，请重新连接。':
    'The Notion integration secret is unavailable. Reconnect to Notion.',
  'Notion 连接无权限，请检查密钥和页面共享设置。':
    'Notion denied access. Check your secret and page sharing settings.',
  'Notion 页面内容已被修改，请手动检查后再导出。':
    'The Notion page was modified. Review it before exporting again.',
  'Notion 页面已被修改，请在 Notion 中检查导出内容。':
    'The Notion page was modified. Review the exported content in Notion.',
  '上次任务中断，已保存的音频可继续整理。':
    'The previous task was interrupted. You can resume processing the saved audio.',
  '上次发送结果待核实，请先核实发送。': 'Verify the result of the previous send before continuing.',
  '下载中断，请重试。': 'Download interrupted. Try again.',
  '下载文件校验失败。': 'Downloaded file verification failed.',
  '下载超时，可重试继续已完成的文件。':
    'Download timed out. Retry to continue with completed files retained.',
  '不支持的导出格式。': 'Unsupported export format.',
  '会议和字幕共用音源，请选择相同的应用或全部系统声音。':
    'Meetings and captions share audio sources. Choose the same app or all system audio.',
  '会议文件读写失败，请检查剩余空间和文件权限。':
    'Could not read or write meeting files. Check free disk space and file permissions.',
  '会议模型加载失败。': 'The meeting model failed to load.',
  '会议模型加载超时。': 'Loading the meeting model timed out.',
  会议逐字稿: 'Meeting transcript',
  '候选纪要过多，请在逐字稿中分段整理。':
    'Too many candidate notes. Process smaller sections of the transcript.',
  '助手正在回答另一段对话，请稍后重试。':
    'The assistant is answering another conversation. Try again shortly.',
  '原文格式无效。': 'Invalid transcript format.',
  '填入结果未知，请先检查目标输入框。':
    'Insertion status is unknown. Check the target text field first.',
  '字幕 · 系统声音': 'Captions · System audio',
  '字幕 · 麦克风': 'Captions · Microphone',
  '字幕外观超出范围。': 'Caption appearance settings are out of range.',
  '字幕已为听写暂停，听写结束后自动恢复。':
    'Captions are paused for dictation and will resume automatically afterward.',
  '字幕识别引擎没有返回结果，请重新开始字幕。':
    'The recognition engine returned no result. Restart captions.',
  '字幕识别组件无法加载，可能是应用签名或组件不完整。请退出后打开新版 SpeakMore。':
    'The caption recognition component could not load. The app signature or installation may be incomplete. Quit and open the latest SpeakMore build.',
  '字幕音源已断开。': 'The caption audio source disconnected.',
  实时字幕: 'Live captions',
  '对话记录不完整，请清空对话后重试。':
    'Conversation history is incomplete. Clear the conversation and try again.',
  '对话记录过大，请清空对话后继续。': 'Conversation history is too large. Clear it to continue.',
  '对话记录过大，请清空对话后重试。': 'Conversation history is too large. Clear it and try again.',
  '导入已停止，请重试。': 'Import stopped. Try again.',
  '导出格式无效。': 'Invalid export format.',
  '尚未确认唯一的目标页面。请在 Notion 中检查，避免重复发送；可先复制纪要。':
    'A unique destination page has not been confirmed. Check Notion to avoid duplicates; you can copy the notes for now.',
  '已为听写暂停，听写结束后继续整理。':
    'Processing is paused for dictation and will resume afterward.',
  '已停止回答，问题仍保留在输入框。': 'Response stopped. Your question remains in the input field.',
  '已停止整理，录音和逐字稿已保留。': 'Processing stopped. Audio and transcript are kept.',
  '应用正在退出，对话已停止。': 'The app is quitting. The conversation has stopped.',
  '开始字幕或打开已保存的字幕后即可提问。':
    'Start captions or open a saved caption session to ask a question.',
  '当前操作不可用。': 'This action is unavailable.',
  '操作已停止，已保存的内容保留在会议记录中。':
    'Operation stopped. Saved content remains in Meetings.',
  '操作无效。': 'Invalid action.',
  '整理模型不完整，请重新下载。': 'The summary model is incomplete. Download it again.',
  '整理模型校验失败，请重新下载。': 'Summary model verification failed. Download it again.',
  '整理输出超出限制。': 'Summary output exceeds the limit.',
  '文字过长。': 'The text is too long.',
  '文字过长或为空，请使用复制。': 'The text is empty or too long to insert. Use Copy instead.',
  '无法保存 Notion 连接密钥。': 'Could not save the Notion integration secret.',
  '无法创建第二路字幕识别，请重新开始。':
    'Could not create the second recognition stream. Restart captions.',
  '无法加载会议转写模型。': 'Could not load the meeting transcription model.',
  '无法向本地翻译引擎发送文字，请重新开始字幕。':
    'Could not send text to the local translation engine. Restart captions.',
  '无法启动 MOSS 环境安装，请重试。': 'Could not start MOSS runtime installation. Try again.',
  '无法启动麦克风。': 'Could not start the microphone.',
  '无法复制，请重试。': 'Could not copy. Try again.',
  '无法开始会议转写。': 'Could not start meeting transcription.',
  '无法打开文件选择器，请重试。': 'Could not open the file picker. Try again.',
  '无法核实页面内容，请稍后重试。': 'Could not verify page content. Try again later.',
  '无法检查 MOSS 安装进度，请重试。': 'Could not check MOSS installation progress. Try again.',
  '无法检查输入框，请复制文字。': 'Could not check the text field. Copy the text instead.',
  '无法解码此音频文件，请尝试 WAV 或 M4A。': 'Could not decode this audio file. Try WAV or M4A.',
  '无法访问 Notion 钥匙串。': 'Could not access the Notion secret in Keychain.',
  '无法读取可采集声音的应用列表，请稍后重试；也可选择「全部系统声音」。':
    'Could not list apps for audio capture. Try again or select All system audio.',
  '无法读取整理引擎状态。': 'Could not read the summary engine status.',
  '无法读取此音频，请使用 WAV、MP3、M4A 或 FLAC。':
    'Could not read this audio. Use WAV, MP3, M4A or FLAC.',
  '无法连接模型下载地址，请检查网络后重试。':
    'Could not connect to the model download server. Check your network and try again.',
  '无法采集系统声音。请在系统设置的「屏幕与系统音频录制」中允许 SpeakMore；修改后重启应用。':
    'Could not capture system audio. Allow SpeakMore in System Settings → Screen & System Audio Recording, then restart the app.',
  '暂未发现可采集声音的应用，请先打开会议或播放器应用，再重试。':
    'No apps available for audio capture. Open a meeting or media app and try again.',
  '未检测到清晰语音。录音已保留，可检查音源后重试。':
    'No clear speech detected. Your recording is kept. Check the audio source and try again.',
  '未确认填入成功，请检查目标输入框；文字仍可复制。':
    'Insertion could not be confirmed. Check the target field; the text can still be copied.',
  '未选择输入框，文字仍在本页，可重试或复制。':
    'No text field selected. Your text remains here; retry or copy it.',
  '本地整理失败，逐字稿已保留。': 'Local summarization failed. The transcript is kept.',
  '本地整理引擎不可用，请安装完整桌面版本。':
    'The local summary engine is unavailable. Install the complete desktop app.',
  '本地文字模型加载失败。': 'The local text model failed to load.',
  '本地翻译失败。': 'Local translation failed.',
  '本地翻译引擎不可用。': 'The local translation engine is unavailable.',
  '本地翻译引擎返回了无效结果，请重新开始字幕。':
    'The local translation engine returned an invalid result. Restart captions.',
  '本地翻译进程退出。': 'The local translation process exited.',
  '本次字幕已达到上下文保存上限，请开始新会话。':
    'This caption session reached its context limit. Start a new session.',
  '本次对话已达到保存上限，请清空对话后继续；原文不受影响。':
    'This conversation reached its storage limit. Clear it to continue; source text is unaffected.',
  '本段整理超时，逐字稿已保留。': 'Summarizing this segment timed out. The transcript is kept.',
  '本段转写失败，可重试继续处理。': 'Transcribing this segment failed. Retry to continue.',
  '本段转写超时，可继续处理已保存的录音。':
    'Transcribing this segment timed out. You can resume processing saved audio.',
  '标题请填写 1–120 个字。': 'Enter a title between 1 and 120 characters.',
  '模型下载服务不可用。': 'The model download service is unavailable.',
  '模型下载服务暂不可用。': 'The model download service is temporarily unavailable.',
  '模型下载超时，请重试。': 'Model download timed out. Try again.',
  '模型下载连接失败。': 'Could not connect to download the model.',
  '模型不存在。': 'Model not found.',
  '模型完整性校验失败，请重新下载。': 'Model integrity verification failed. Download it again.',
  '模型完整性校验失败，请重试。': 'Model integrity verification failed. Try again.',
  '模型文件大小不匹配。': 'The model file size does not match.',
  '模型未生成完整的纪要，请重试。': 'The model did not generate complete notes. Try again.',
  '正在完成最后一句翻译…': 'Finishing the last translation…',
  '没有可转写的音频，请录音或导入文件。': 'No audio to transcribe. Record or import a file.',
  '流式模型加载失败。': 'The streaming model failed to load.',
  '目标输入框不可用，请将光标放入可编辑区域后重试。':
    'The target text field is unavailable. Place the cursor in an editable area and try again.',
  '窗口不可用。': 'This window is unavailable.',
  '系统睡眠或会话切换，字幕已停止。':
    'Captions stopped because the system slept or the session changed.',
  '系统睡眠或会话切换，录音已中断；已保存的音频可继续整理。':
    'Recording was interrupted by system sleep or a session change. Saved audio can still be processed.',
  '系统音频组件加载失败，请重新打开新版 SpeakMore。':
    'The system audio component failed to load. Reopen the latest SpeakMore build.',
  '纪要条目过多，请缩短输入后重试。': 'Too many note items. Shorten the input and try again.',
  '纪要的原文依据或内容格式无效，已保留逐字稿。':
    'The notes contain invalid evidence or formatting. The transcript is kept.',
  '网络中断，发送结果待核实。请点击核实发送，避免重复创建。':
    'Network interrupted. Verify the send result before retrying to avoid duplicates.',
  '翻译任务中断，已保留原文。': 'Translation interrupted. Original text is kept.',
  '翻译延迟较高，已跳过过时译文；原文仍保留。':
    'Translation is falling behind. Outdated translations were skipped; original text is kept.',
  '翻译服务返回内容无效，已保留原文。':
    'The translation service returned invalid content. Original text is kept.',
  '翻译耗时过长，已保留原文。请使用更快的本地模型或停止其他高负载任务。':
    'Translation took too long. Original text is kept. Use a faster local model or stop other demanding tasks.',
  '翻译队列已满，本句仅显示原文。':
    'The translation queue is full. Only the original text is shown for this sentence.',
  '解码音频超过 60 分钟。': 'Decoded audio exceeds 60 minutes.',
  '识别延迟较高，已停止字幕，避免内容错位。':
    'Recognition is falling behind. Captions stopped to avoid misaligned content.',
  '识别结果过长，请重新开始字幕。': 'Recognition output is too long. Restart captions.',
  '请先下载会议整理模型。': 'Download the meeting summary model first.',
  '请先下载字幕使用的流式识别模型。': 'Download a streaming speech model for captions first.',
  '请先在语音模型中下载所选模型。': 'Download the selected model in Voice models first.',
  '请先连接 Notion。': 'Connect to Notion first.',
  '请在系统设置 → 隐私与安全性 → 屏幕与系统音频录制中允许 SpeakMore。授权后如仍无法读取，请退出并重新打开应用。':
    'Allow SpeakMore in System Settings → Privacy & Security → Screen & System Audio Recording. If access still fails, quit and reopen the app.',
  '请在设置 → 语音模型中下载 MOSS 模型，再继续整理。':
    'Download the MOSS model in Settings → Voice models before continuing.',
  '请在设置 → 语音模型中安装 MOSS 运行环境，再继续整理。':
    'Install the MOSS runtime in Settings → Voice models before continuing.',
  '请在设置中授权麦克风。': 'Allow microphone access in System Settings.',
  '请在语音模型中下载并选择转写模型，再继续整理。':
    'Download and select a transcription model in Voice models before continuing.',
  '请填写 Notion 父页面的 32 位页面 ID。': 'Enter the 32-character ID of the Notion parent page.',
  '请填写有效的 Notion 内部连接密钥。': 'Enter a valid Notion internal integration secret.',
  '请至少开启一个音频来源。': 'Enable at least one audio source.',
  '请输入问题，最多约 500 个汉字。':
    'Enter a question under 1,500 UTF-8 bytes (about 1,500 English characters).',
  '请选择不超过 2 GB 的音频文件。': 'Choose an audio file no larger than 2 GB.',
  '请选择中文、英文或自动识别。': 'Choose Chinese, English or Auto-detect.',
  '请选择保留期限。': 'Choose a retention period.',
  '请选择字幕翻译的目标语言。': 'Choose a target language for caption translation.',
  '请选择已配置的 AI 服务，或下载本地 Qwen3 用于翻译。':
    'Choose a configured AI service or download local Qwen3 for translation.',
  '请选择录音来源。': 'Choose a recording source.',
  '请选择文件内最多 60 分钟的区间。': 'Choose a section of the file up to 60 minutes long.',
  '请选择有效的音频来源和模型。': 'Choose a valid audio source and model.',
  '请选择要导入的文件。': 'Choose a file to import.',
  '转写引擎不可用。': 'The transcription engine is unavailable.',
  '运行环境下载超时，可重试继续已完成的文件。':
    'Runtime download timed out. Retry to continue with completed files retained.',
  '运行环境文件大小不符，请重试。': 'The runtime file size does not match. Try again.',
  '运行环境文件校验失败，请重试。': 'Runtime file verification failed. Try again.',
  '这句话未完成翻译，原文已保留。':
    'This sentence was not fully translated. Original text is kept.',
  '逐字稿已完成。下载本地整理模型后可生成纪要。':
    'The transcript is ready. Download the local summary model to generate notes.',
  '音源中断或处理跟不上，已保存的内容保留。':
    'The audio source disconnected or processing fell behind. Saved content is kept.',
  '音频处理跟不上或音源已中断，已停止以避免生成缺失的记录。':
    'Capture stopped because audio processing fell behind or the source disconnected, preventing incomplete records.',
  '音频时长无效。': 'Invalid audio duration.',
  '页面内容与导出记录不一致，请手动检查。':
    'Page content differs from the export record. Review it manually.',
  '麦克风录音中断，已保存的音频保留。':
    'Microphone recording was interrupted. Saved audio is kept.',
  '会议录音还剩 1 分钟，将自动保存。':
    'One minute of meeting recording remains. It will be saved automatically.',
  '会议录音还剩 5 分钟。': 'Five minutes of meeting recording remain.',
  '问题有些长，请缩短到约 500 个汉字。':
    'Your question is too long. Keep it under 1,500 UTF-8 bytes (about 1,500 English characters).',
  '请在 SpeakMore 桌面应用中使用会议功能。':
    'Open the SpeakMore desktop app to use meeting features.',
  '操作未完成，请重试。': 'The operation did not finish. Please try again.',
  '会议内容已更新，请关闭此窗口后重新打开再修改。':
    'This meeting changed after you opened this window. Close it and open it again to make changes.',
  '这场会议已不存在，可能已被删除。': 'This meeting no longer exists. It may have been deleted.',
}

export function meetingMessage(message: string | null | undefined, t: Translate): string {
  if (!message) return ''
  return t(message, english[message] ?? message)
}

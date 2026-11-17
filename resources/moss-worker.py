"""Local-only, bounded MOSS inference. Model classes ship with the application."""
import os
os.environ.update(HF_HUB_OFFLINE="1", TRANSFORMERS_OFFLINE="1", HF_HUB_DISABLE_TELEMETRY="1", TOKENIZERS_PARALLELISM="false")
import json, sys, wave, tempfile, time, threading
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parent))
from speaker_identity import Identities, Extractor
sys.path.insert(0, str(Path(__file__).resolve().parent / 'moss-package'))
import numpy as np
import torch
from moss_transcribe_diarize import MossTranscribeDiarizeForConditionalGeneration, MossTranscribeDiarizeProcessor, parse_transcript
from moss_transcribe_diarize.inference_utils import build_transcription_messages, generate_transcription

def exit_with_parent():
    # The app holds stdin open; EOF means it exited or crashed, so never linger as an orphan.
    try: sys.stdin.buffer.read()
    finally: os._exit(0)

def main():
    threading.Thread(target=exit_with_parent, daemon=True).start()
    model_dir, meeting_dir = map(Path, sys.argv[1:3])
    live = len(sys.argv) > 5 and sys.argv[5] == '--live'
    meeting = json.loads((meeting_dir / 'meeting.json').read_text())
    identities = Identities(meeting.get('speakerProfiles', []))
    extractor = Extractor(Path(sys.argv[3]), Path(sys.argv[4]))
    torch.set_num_threads(4)
    device = torch.device("mps" if torch.backends.mps.is_available() else "cpu")
    model = MossTranscribeDiarizeForConditionalGeneration.from_pretrained(str(model_dir), local_files_only=True, dtype=torch.float32, attn_implementation='sdpa').to(device).eval()
    processor = MossTranscribeDiarizeProcessor.from_pretrained(str(model_dir), local_files_only=True, trust_remote_code=False)
    total = sum(c['samples'] for c in meeting['chunks'])
    position = meeting['processedSamples']
    print(json.dumps({'ready': True}), flush=True)
    while True:
        if live:
            meeting = json.loads((meeting_dir / 'meeting.json').read_text())
            total = sum(c['samples'] for c in meeting['chunks'])
            if total - position < 60 * 16000:
                print(json.dumps({'waiting': True}), flush=True)
                time.sleep(5)
                continue
        elif position >= total:
            break
        end = min(total, position + 60 * 16000)
        pieces=[]; offset=0
        for chunk in meeting['chunks']:
            size=chunk['samples']; first=max(0,position-offset);last=min(size,end-offset)
            if first<last:
                with (meeting_dir/f"{chunk['index']:05}.pcm").open('rb') as f:
                    f.seek(first*4); data=f.read((last-first)*4)
                if len(data)!=(last-first)*4:raise ValueError('Truncated audio')
                pieces.append(np.frombuffer(data,dtype='<f4'))
            offset+=size
            if offset>=end:break
        audio=np.concatenate(pieces)
        with tempfile.NamedTemporaryFile(suffix='.wav', dir=meeting_dir) as tmp:
            with wave.open(tmp.name,'wb') as w:
                w.setnchannels(1);w.setsampwidth(2);w.setframerate(16000);w.writeframes((np.clip(audio,-1,1)*32767).astype('<i2').tobytes())
            with torch.inference_mode():
                result=generate_transcription(model,processor,build_transcription_messages(tmp.name),max_new_tokens=4096,do_sample=False,device=device,dtype=torch.float32)
        if result.get("generated_tokens",0) >= 4096: raise ValueError("Output token limit")
        segments=[]
        parsed = list(parse_transcript(result['text']))
        voices = {}
        for seg in parsed:
            if not seg.speaker: continue
            first, last = max(0,int(float(seg.start)*16000)), min(len(audio),int(float(seg.end)*16000))
            # Overlapping voices cannot supply clean identity evidence.
            if first >= last or any(other.speaker != seg.speaker and float(other.start) < float(seg.end) and float(other.end) > float(seg.start) for other in parsed): continue
            voices.setdefault(str(seg.speaker), []).append(audio[first:last])
        embeddings = {str(seg.speaker): None for seg in parsed}
        for label, pieces in voices.items():
            embeddings[label] = extractor.embed(np.concatenate(pieces))
        identities_for_window = identities.assign(embeddings, position//960000+1)
        for seg in parsed:
            start=int(float(seg.start)*1000);finish=int(float(seg.end)*1000)
            if not 0 <= start < finish <= len(audio)//16+1000:continue
            start_ms,end_ms=position//16+start,min(end//16,position//16+finish)
            if start_ms>=end_ms:continue
            identity, name = identities_for_window[str(seg.speaker)]
            segments.append(dict(startMs=start_ms,endMs=end_ms,text=seg.text,speaker=name,speakerId=identity))
        print(json.dumps({'processed':end,'segments':segments,'profiles':identities.profiles},ensure_ascii=False),flush=True)
        position=end
    extractor.close()
if __name__=='__main__':
    try:main()
    except Exception:
        print(json.dumps({'error':'MOSS local inference failed'}),flush=True);sys.exit(1)

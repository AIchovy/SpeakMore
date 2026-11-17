"""Within-meeting voice identity. Labels never determine identity; embeddings do."""
import ctypes
import math


def unit(vector):
    norm = math.sqrt(sum(v * v for v in vector))
    return [v / norm for v in vector] if norm > 1e-9 else None


class Identities:
    def __init__(self, profiles=None):
        self.profiles = profiles or []

    def assign(self, embeddings, window):
        assigned, used = {}, set()
        # Strong evidence first; no two window labels are silently merged.
        ranked = []
        for label, vector in embeddings.items():
            vector = unit(vector) if vector else None
            scores = {}
            if vector:
                for p in self.profiles:
                    if len(vector) == len(p['embedding']):
                        score = sum(a * b for a, b in zip(vector, p['embedding']))
                        scores[p['id']] = max(score, scores.get(p['id'], -1))
            candidates = sorted(scores.items(), key=lambda v: v[1], reverse=True)
            ranked.append((candidates[0][1] if candidates else -1, label, vector, candidates))
        for _, label, vector, candidates in sorted(ranked, reverse=True):
            if vector is None:
                assigned[label] = (f'uncertain-{window}-{label}', f'待确认 · 片段 {window} · {label}')
                continue
            best = candidates[0] if candidates else None
            margin = best[1] - candidates[1][1] if len(candidates) > 1 else 1
            if best and best[1] >= 0.70 and margin >= 0.06 and best[0] not in used:
                identity = best[0]
                # Preserve multiple user-merged profiles; update nearest voice only.
                p = max((p for p in self.profiles if p['id'] == identity),
                        key=lambda p: sum(a*b for a,b in zip(vector,p['embedding'])))
                weight = min(p['count'], 20)
                p['embedding'] = unit([a * weight + b for a, b in zip(p['embedding'], vector)])
                p['count'] += 1
            elif best and best[1] >= 0.60:
                assigned[label] = (f'uncertain-{window}-{label}', f'待确认 · 片段 {window} · {label}')
                continue
            else:
                number = max([int(p['id'][1:]) for p in self.profiles if p['id'].startswith('p') and p['id'][1:].isdigit()] + [0]) + 1
                identity = f'p{number}'
                if len(self.profiles) >= 128:
                    assigned[label] = (f'uncertain-{window}-{label}', f'待确认 · 片段 {window} · {label}')
                    continue
                self.profiles.append(dict(id=identity, embedding=vector, count=1))
            used.add(identity)
            assigned[label] = (identity, f'说话人 {identity[1:]}' if identity.startswith('p') else identity)
        return assigned


class Extractor:
    def __init__(self, model, library):
        self.lib = ctypes.CDLL(str(library))
        self.lib.sm_speaker_create.argtypes = [ctypes.c_char_p]
        self.lib.sm_speaker_create.restype = ctypes.c_void_p
        self.lib.sm_speaker_dim.argtypes = [ctypes.c_void_p]
        self.lib.sm_speaker_embedding.argtypes = [ctypes.c_void_p, ctypes.POINTER(ctypes.c_float), ctypes.c_int, ctypes.POINTER(ctypes.c_float), ctypes.c_int]
        self.lib.sm_speaker_destroy.argtypes = [ctypes.c_void_p]
        self.raw = self.lib.sm_speaker_create(str(model).encode())
        if not self.raw:
            raise ValueError('Speaker extractor unavailable')
        self.dim = self.lib.sm_speaker_dim(self.raw)
        if not 0 < self.dim <= 512:
            self.close()
            raise ValueError('Speaker dimensions')

    def embed(self, samples):
        if len(samples) < 16000:  # Short utterances remain explicitly uncertain.
            return None
        import numpy as np
        # Hand the contiguous float32 buffer to C directly instead of boxing every sample.
        samples = np.ascontiguousarray(samples[:480000], dtype=np.float32)
        pcm = samples.ctypes.data_as(ctypes.POINTER(ctypes.c_float))
        result = (ctypes.c_float * self.dim)()
        if self.lib.sm_speaker_embedding(self.raw, pcm, len(samples), result, self.dim) != self.dim:
            return None
        vector = list(result)
        return vector if all(math.isfinite(v) for v in vector) else None

    def close(self):
        if self.raw:
            self.lib.sm_speaker_destroy(self.raw)
            self.raw = None

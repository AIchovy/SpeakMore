#include "c-api.h"
#include <memory>
#include <string>
#include <algorithm>

struct Model {
    const SherpaOnnxOnlineRecognizer *value;
    ~Model() { SherpaOnnxDestroyOnlineRecognizer(value); }
};
struct Recognizer {
    std::shared_ptr<Model> model;
    const SherpaOnnxOnlineStream *stream;
    std::string result;
};
static Recognizer *create_stream(const std::shared_ptr<Model> &model) {
    const auto *stream = SherpaOnnxCreateOnlineStream(model->value);
    return stream ? new Recognizer{model, stream, {}} : nullptr;
}
extern "C" void *sm_asr_create(const char *root, int paraformer) {
    std::string base(root), encoder=base+(paraformer?"/encoder.int8.onnx":"/encoder-epoch-99-avg-1.int8.onnx"), decoder=base+(paraformer?"/decoder.int8.onnx":"/decoder-epoch-99-avg-1.onnx"), joiner=base+"/joiner-epoch-99-avg-1.int8.onnx", tokens=base+"/tokens.txt";
    SherpaOnnxOnlineRecognizerConfig c{};
    c.feat_config.sample_rate=16000; c.feat_config.feature_dim=80;
    c.model_config.tokens=tokens.c_str(); c.model_config.num_threads=2; c.model_config.provider="cpu";
    if(paraformer) { c.model_config.paraformer.encoder=encoder.c_str(); c.model_config.paraformer.decoder=decoder.c_str(); }
    else {c.model_config.transducer.encoder=encoder.c_str();c.model_config.transducer.decoder=decoder.c_str();c.model_config.transducer.joiner=joiner.c_str();}
    c.decoding_method="greedy_search"; c.enable_endpoint=1;
    c.rule1_min_trailing_silence=2.0; c.rule2_min_trailing_silence=0.65; c.rule3_min_utterance_length=8.0;
    const auto *r=SherpaOnnxCreateOnlineRecognizer(&c); if(!r)return nullptr;
    auto model = std::make_shared<Model>(); model->value = r;
    return create_stream(model);
}
// Audio tracks share immutable model weights, but keep separate decoding/endpoint state.
// All calls remain on the single caption capture thread.
extern "C" void *sm_asr_fork(void *raw) {
    return raw ? create_stream(static_cast<Recognizer*>(raw)->model) : nullptr;
}
extern "C" const char *sm_asr_accept(void *raw,const float *pcm,int count,int *final) {
    if(!raw || !final || count < 0 || (count > 0 && !pcm)) return nullptr;
    auto *r=static_cast<Recognizer*>(raw);
    if(!r->stream)return nullptr;
    if(count>0)SherpaOnnxOnlineStreamAcceptWaveform(r->stream,16000,pcm,count);
    else {float tail[1600]{};SherpaOnnxOnlineStreamAcceptWaveform(r->stream,16000,tail,1600);SherpaOnnxOnlineStreamInputFinished(r->stream);}
    while(SherpaOnnxIsOnlineStreamReady(r->model->value,r->stream))SherpaOnnxDecodeOnlineStream(r->model->value,r->stream);
    const auto *result=SherpaOnnxGetOnlineStreamResult(r->model->value,r->stream);
    r->result=result&&result->text?result->text:"";
    if(result)SherpaOnnxDestroyOnlineRecognizerResult(result);
    *final=count==0||SherpaOnnxOnlineStreamIsEndpoint(r->model->value,r->stream);
    if(count==0){SherpaOnnxDestroyOnlineStream(r->stream);r->stream=SherpaOnnxCreateOnlineStream(r->model->value);}
    else if(*final)SherpaOnnxOnlineStreamReset(r->model->value,r->stream);
    return r->result.c_str();
}
extern "C" void sm_asr_destroy(void *raw) {
    if(!raw)return;
    auto*r=static_cast<Recognizer*>(raw);
    if(r->stream)SherpaOnnxDestroyOnlineStream(r->stream);
    delete r;
}

extern "C" void *sm_speaker_create(const char *model) {
    if (!model) return nullptr;
    SherpaOnnxSpeakerEmbeddingExtractorConfig config{};
    config.model = model; config.num_threads = 1; config.provider = "cpu";
    return const_cast<SherpaOnnxSpeakerEmbeddingExtractor*>(SherpaOnnxCreateSpeakerEmbeddingExtractor(&config));
}
extern "C" int sm_speaker_dim(void *raw) {
    return raw ? SherpaOnnxSpeakerEmbeddingExtractorDim(static_cast<const SherpaOnnxSpeakerEmbeddingExtractor*>(raw)) : 0;
}
extern "C" int sm_speaker_embedding(void *raw, const float *pcm, int count, float *output, int capacity) {
    if (!raw || !pcm || !output || count < 8000 || count > 480000) return 0;
    const auto *extractor = static_cast<const SherpaOnnxSpeakerEmbeddingExtractor*>(raw);
    const int dim = SherpaOnnxSpeakerEmbeddingExtractorDim(extractor);
    if (dim <= 0 || dim > capacity) return 0;
    const auto *stream = SherpaOnnxSpeakerEmbeddingExtractorCreateStream(extractor);
    if (!stream) return 0;
    SherpaOnnxOnlineStreamAcceptWaveform(stream, 16000, pcm, count);
    SherpaOnnxOnlineStreamInputFinished(stream);
    const float *embedding = SherpaOnnxSpeakerEmbeddingExtractorIsReady(extractor, stream)
        ? SherpaOnnxSpeakerEmbeddingExtractorComputeEmbedding(extractor, stream) : nullptr;
    if (embedding) std::copy(embedding, embedding + dim, output);
    if (embedding) SherpaOnnxSpeakerEmbeddingExtractorDestroyEmbedding(embedding);
    SherpaOnnxDestroyOnlineStream(stream);
    return embedding ? dim : 0;
}
extern "C" void sm_speaker_destroy(void *raw) {
    if (raw) SherpaOnnxDestroySpeakerEmbeddingExtractor(static_cast<const SherpaOnnxSpeakerEmbeddingExtractor*>(raw));
}

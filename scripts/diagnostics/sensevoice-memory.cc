// Developer-only controlled experiment using the pinned public C API.
// The Python runner verifies native/model inputs and owns all output paths.
#include "c-api.h"
#ifdef SPEAKMORE_MEMORY_ARENA_EXPERIMENT
#include "onnxruntime_c_api.h"
#endif
#include <libproc.h>
#include <sys/resource.h>
#include <unistd.h>
#include <chrono>
#include <fstream>
#include <iostream>
#include <memory>
#include <stdexcept>
#include <string>
#include <vector>

static void memory(const char *stage, long long elapsed) {
  struct rusage usage = {};
  struct rusage_info_v4 detail = {};
  if (getrusage(RUSAGE_SELF, &usage) != 0 ||
      proc_pid_rusage(getpid(), RUSAGE_INFO_V4, (rusage_info_t *)&detail) != 0)
    throw std::runtime_error("metrics unavailable");
  std::cout << "{\"stage\":\"" << stage << "\",\"elapsed_ms\":" << elapsed
            << ",\"rss_bytes\":" << detail.ri_resident_size
            << ",\"max_rss_bytes\":" << usage.ru_maxrss
            << ",\"footprint_bytes\":" << detail.ri_phys_footprint
            << ",\"peak_footprint_bytes\":" << detail.ri_lifetime_max_phys_footprint
            << "}" << std::endl;
}

int main(int argc, char **argv) try {
  if (argc != 8) return 64;
  if (std::string(SherpaOnnxGetVersionStr()) != "1.13.8" ||
      std::string(SherpaOnnxGetOnnxruntimeVersionStr()) != "1.28.2") return 65;
  auto started = std::chrono::steady_clock::now();
  auto elapsed = [&] {return std::chrono::duration_cast<std::chrono::milliseconds>(
      std::chrono::steady_clock::now() - started).count();};
  auto wave = std::unique_ptr<const SherpaOnnxWave, decltype(&SherpaOnnxFreeWave)>(
      SherpaOnnxReadWave(argv[3]), SherpaOnnxFreeWave);
  if (!wave || wave->sample_rate != 16000 || wave->num_samples <= 0) return 65;
  const int repeats = std::stoi(argv[6]);
  if (repeats < 1 || repeats > 3) return 64;
  std::vector<float> phrase(wave->samples, wave->samples + wave->num_samples);
  phrase.resize(phrase.size() + 16000, 0.0f);
  std::vector<float> samples(4800000);
  for (size_t i = 0; i < samples.size(); ++i) samples[i] = phrase[i % phrase.size()];
  wave.reset();
#ifdef SPEAKMORE_MEMORY_ARENA_EXPERIMENT
  const auto *api = OrtGetApiBase()->GetApi(ORT_API_VERSION);
  if (!api) return 65;
  auto check = [api](OrtStatus *status) {
    if (status) {
      api->ReleaseStatus(status);
      throw std::runtime_error("ORT setup failed");
    }
  };
  std::unique_ptr<OrtEnv, decltype(api->ReleaseEnv)> environment(nullptr, api->ReleaseEnv);
  const std::string mode(argv[7]);
  if (mode == "exact_arena" || mode == "compact_arena" || mode == "growth_arena") {
    OrtEnv *env = nullptr;
    check(api->CreateEnv(ORT_LOGGING_LEVEL_ERROR, "memory-study", &env));
    environment.reset(env);
    OrtMemoryInfo *info = nullptr;
    check(api->CreateCpuMemoryInfo(OrtArenaAllocator, OrtMemTypeDefault, &info));
    std::unique_ptr<OrtMemoryInfo, decltype(api->ReleaseMemoryInfo)> memory_info(info, api->ReleaseMemoryInfo);
    const char *keys[] = {"arena_extend_strategy", "max_dead_bytes_per_chunk", "max_power_of_two_extend_bytes"};
    const size_t values[] = {mode == "growth_arena" ? size_t(0) : size_t(1), 1024 * 1024, 32 * 1024 * 1024};
    OrtArenaCfg *cfg = nullptr;
    check(api->CreateArenaCfgV2(keys, values, mode == "exact_arena" ? 1 : 3, &cfg));
    std::unique_ptr<OrtArenaCfg, decltype(api->ReleaseArenaCfg)> arena(cfg, api->ReleaseArenaCfg);
    check(api->CreateAndRegisterAllocator(env, info, cfg));
  }
#endif
  SherpaOnnxOfflineRecognizerConfig config = {};
  config.feat_config.sample_rate = 16000;
  config.feat_config.feature_dim = 80;
  config.model_config.num_threads = 4;
  config.model_config.provider = argv[4];
  config.model_config.model_type = "sense_voice";
  config.model_config.tokens = argv[2];
  config.model_config.sense_voice.model = argv[1];
  config.model_config.sense_voice.language = "en";
  config.model_config.sense_voice.use_itn = 1;
  config.decoding_method = "greedy_search";
  auto recognizer = std::unique_ptr<const SherpaOnnxOfflineRecognizer,
      decltype(&SherpaOnnxDestroyOfflineRecognizer)>(
      SherpaOnnxCreateOfflineRecognizer(&config), SherpaOnnxDestroyOfflineRecognizer);
  if (!recognizer) return 66;
  memory("loaded", elapsed());
  for (int iteration = 0; iteration < repeats; ++iteration) {
    auto stream = std::unique_ptr<const SherpaOnnxOfflineStream,
        decltype(&SherpaOnnxDestroyOfflineStream)>(
        SherpaOnnxCreateOfflineStream(recognizer.get()), SherpaOnnxDestroyOfflineStream);
    if (!stream) return 66;
    SherpaOnnxAcceptWaveformOffline(stream.get(), 16000, samples.data(), samples.size());
    const auto before = elapsed();
    SherpaOnnxDecodeOfflineStream(recognizer.get(), stream.get());
    auto result = std::unique_ptr<const SherpaOnnxOfflineRecognizerResult,
        decltype(&SherpaOnnxDestroyOfflineRecognizerResult)>(
        SherpaOnnxGetOfflineStreamResult(stream.get()), SherpaOnnxDestroyOfflineRecognizerResult);
    if (!result || !result->text || !result->lang || std::string(result->lang) != "<|en|>") return 66;
    std::ofstream output(std::string(argv[5]) + "." + std::to_string(iteration));
    output << result->text;
    output.close(); if (!output) return 74;
    memory("decoded", elapsed() - before);
    result.reset(); stream.reset();
    memory("released", elapsed());
  }
  recognizer.reset();
#ifdef SPEAKMORE_MEMORY_ARENA_EXPERIMENT
  environment.reset();
#endif
  memory("unloaded", elapsed());
  return 0;
} catch (const std::exception &) {
  std::cerr << "MemoryExperimentFailed\n"; return 70;
}

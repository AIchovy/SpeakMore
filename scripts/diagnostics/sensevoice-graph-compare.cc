// Offline native comparison of complete public waveforms; no product IPC or data.
#include "c-api.h"
#include <libproc.h>
#include <sys/resource.h>
#include <unistd.h>
#include <chrono>
#include <fstream>
#include <iostream>
#include <map>
#include <memory>
#include <stdexcept>
#include <string>
#include <vector>

static void memory(const std::string &name, size_t samples, long long elapsed) {
  struct rusage usage = {};
  struct rusage_info_v4 detail = {};
  if (getrusage(RUSAGE_SELF, &usage) != 0 ||
      proc_pid_rusage(getpid(), RUSAGE_INFO_V4, (rusage_info_t *)&detail) != 0)
    throw std::runtime_error("metrics unavailable");
  std::cout << "{\"case\":\"" << name << "\",\"samples\":" << samples
            << ",\"decode_ms\":" << elapsed
            << ",\"rss_bytes\":" << detail.ri_resident_size
            << ",\"max_rss_bytes\":" << usage.ru_maxrss
            << ",\"footprint_bytes\":" << detail.ri_phys_footprint
            << ",\"peak_footprint_bytes\":" << detail.ri_lifetime_max_phys_footprint
            << "}" << std::endl;
}

int main(int argc, char **argv) try {
  if (argc != 5) return 64;
  if (std::string(SherpaOnnxGetVersionStr()) != "1.13.8" ||
      std::string(SherpaOnnxGetOnnxruntimeVersionStr()) != "1.28.2") return 65;
  const std::string root(argv[3]);
  const std::vector<std::string> languages = {"zh", "en", "yue", "ja", "ko"};
  std::map<std::string, std::vector<float>> waves;
  for (const auto &language : languages) {
    auto wave = std::unique_ptr<const SherpaOnnxWave, decltype(&SherpaOnnxFreeWave)>(
        SherpaOnnxReadWave((root + "/test_wavs/" + language + ".wav").c_str()), SherpaOnnxFreeWave);
    if (!wave || wave->sample_rate != 16000 || wave->num_samples < 4000) return 65;
    waves[language] = std::vector<float>(wave->samples, wave->samples + wave->num_samples);
  }
  SherpaOnnxOfflineRecognizerConfig config = {};
  config.feat_config.sample_rate = 16000;
  config.feat_config.feature_dim = 80;
  config.model_config.num_threads = 4;
  config.model_config.provider = "cpu";
  config.model_config.model_type = "sense_voice";
  config.model_config.tokens = argv[2];
  config.model_config.sense_voice.model = argv[1];
  config.model_config.sense_voice.language = "auto";
  config.model_config.sense_voice.use_itn = 1;
  config.decoding_method = "greedy_search";
  auto recognizer = std::unique_ptr<const SherpaOnnxOfflineRecognizer,
      decltype(&SherpaOnnxDestroyOfflineRecognizer)>(
      SherpaOnnxCreateOfflineRecognizer(&config), SherpaOnnxDestroyOfflineRecognizer);
  if (!recognizer) return 66;
  memory("loaded", 0, 0);
  auto decode = [&](const std::string &name, const std::vector<float> &samples, const char *language) {
    config.model_config.sense_voice.language = language;
    SherpaOnnxOfflineRecognizerSetConfig(recognizer.get(), &config);
    auto stream = std::unique_ptr<const SherpaOnnxOfflineStream, decltype(&SherpaOnnxDestroyOfflineStream)>(
        SherpaOnnxCreateOfflineStream(recognizer.get()), SherpaOnnxDestroyOfflineStream);
    if (!stream) throw std::runtime_error("stream failed");
    SherpaOnnxAcceptWaveformOffline(stream.get(), 16000, samples.data(), samples.size());
    auto started = std::chrono::steady_clock::now();
    SherpaOnnxDecodeOfflineStream(recognizer.get(), stream.get());
    const auto elapsed = std::chrono::duration_cast<std::chrono::milliseconds>(
        std::chrono::steady_clock::now() - started).count();
    auto result = std::unique_ptr<const SherpaOnnxOfflineRecognizerResult,
        decltype(&SherpaOnnxDestroyOfflineRecognizerResult)>(
        SherpaOnnxGetOfflineStreamResult(stream.get()), SherpaOnnxDestroyOfflineRecognizerResult);
    if (!result || !result->json || !result->text || !*result->text || !result->lang || result->count < 1)
      throw std::runtime_error("empty public speech result");
    std::ofstream out(std::string(argv[4]) + "/" + name + ".json");
    out << result->json;
    out.close(); if (!out) throw std::runtime_error("output failed");
    result.reset(); stream.reset();
    memory(name, samples.size(), elapsed);
  };
  for (const auto &language : languages) {
    decode("short-" + language + "-explicit", waves.at(language), language.c_str());
    decode("short-" + language + "-auto", waves.at(language), "auto");
  }
  auto repeat = [](std::vector<float> pattern) {
    pattern.resize(pattern.size() + 16000, 0.0f);
    std::vector<float> samples(4800000);
    for (size_t i = 0; i < samples.size(); ++i) samples[i] = pattern[i % pattern.size()];
    return samples;
  };
  decode("long-zh", repeat(waves.at("zh")), "zh");
  std::vector<float> mixed;
  for (const auto &language : languages) {
    const auto &wave = waves.at(language);
    mixed.insert(mixed.end(), wave.begin(), wave.end());
    mixed.resize(mixed.size() + 16000, 0.0f);
  }
  decode("long-mixed", repeat(std::move(mixed)), "auto");
  decode("short-en-after-long", waves.at("en"), "en");
  recognizer.reset();
  memory("unloaded", 0, 0);
  return 0;
} catch (const std::exception &) {
  std::cerr << "GraphComparisonFailed\n"; return 70;
}

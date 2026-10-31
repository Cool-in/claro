#include <ctranslate2/translator.h>
#include <nlohmann/json.hpp>
#include <sentencepiece_processor.h>
#include <algorithm>
#include <iostream>
#include <iterator>
#include <memory>
#include <string>
#include <vector>

struct TranslationError { int code; };
class Engine {
  sentencepiece::SentencePieceProcessor tokenizer;
  std::unique_ptr<ctranslate2::Translator> translator;
public:
  explicit Engine(const std::string& model) {
    if (!tokenizer.Load(model + "/sentencepiece.bpe.model").ok()) throw TranslationError{3};
    ctranslate2::ReplicaPoolConfig pool;
    pool.num_threads_per_replica = 4;
    translator = std::make_unique<ctranslate2::Translator>(model, ctranslate2::Device::CPU,
                       ctranslate2::ComputeType::INT8, std::vector<int>{0}, false, pool);
  }
  std::string translate(const std::string& input, const std::string& target, const std::string& source) {
    if (target.size()!=2 || source.size()!=2 || input.empty() || input.size()>12000) throw TranslationError{2};
    if (source==target) return input;
    ctranslate2::TranslationOptions options;
    options.beam_size = 2;
    options.max_decoding_length = 512;
    options.max_input_length = 512;
    std::string translated;
    size_t begin = 0;
    bool first = true;
    while (begin < input.size()) {
      size_t end = input.find('\n', begin);
      const std::string line = input.substr(begin, end == std::string::npos ? end : end - begin);
      if (!first) translated += '\n';
      first = false;
      if (!line.empty()) {
        std::vector<std::string> pieces;
        if (!tokenizer.Encode(line, &pieces).ok()) throw TranslationError{4};
        if (pieces.size()>510) throw TranslationError{7}; // Never silently truncate.
        // Required M2M100 special tokens are not supplied by SentencePiece.
        pieces.insert(pieces.begin(), "__" + source + "__");
        pieces.push_back("</s>");
        const auto results = translator->translate_batch({pieces}, {{"__" + target + "__"}}, options);
        if (results.empty() || results[0].hypotheses.empty()) throw TranslationError{5};
        auto output = results[0].hypotheses[0];
        if (output.size()>=options.max_decoding_length) throw TranslationError{7};
        output.erase(std::remove_if(output.begin(), output.end(), [](const auto& token) {
          return token=="</s>" || token=="<s>" || (token.size()==6 && token.substr(0,2)=="__");
        }), output.end());
        std::string decoded;
        if (!tokenizer.Decode(output, &decoded).ok()) throw TranslationError{6};
        translated += decoded;
      }
      if (end==std::string::npos) break;
      begin=end+1;
    }
    if (!input.empty() && input.back()=='\n') translated+='\n';
    return translated;
  }
};
// JSON lines are private process IPC, never written to logs or history.
// The desktop process closes stdin or kills this worker after 60 idle seconds.
int main(int argc, char** argv) {
  try {
    if (argc==3 && std::string(argv[1])=="--server") {
      Engine engine(argv[2]);
      // Prime decoder allocations before announcing readiness; no user text is involved.
      engine.translate("Hello.","es","en");
      std::cout<<"{\"ready\":true}"<<std::endl;
      std::string line;
      while (std::getline(std::cin,line)) {
        nlohmann::json response={{"id",0},{"code",0}};
        try {
          const auto request=nlohmann::json::parse(line);
          response["id"]=request.at("id");
          response["translation"]=engine.translate(request.at("text"),request.at("target"),request.at("source"));
        } catch (const TranslationError& error) { response["code"]=error.code; }
          catch (...) { response["code"]=1; }
        std::cout<<response.dump()<<std::endl;
      }
      return 0;
    }
    if (argc!=4) return 2;
    const std::string input((std::istreambuf_iterator<char>(std::cin)),{});
    Engine engine(argv[1]);
    std::cout<<engine.translate(input,argv[2],argv[3]);
    return 0;
  } catch (const TranslationError& error) { return error.code; }
    catch (...) { return 1; }
}

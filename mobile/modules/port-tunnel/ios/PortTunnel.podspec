Pod::Spec.new do |s|
  s.name = 'PortTunnel'
  s.version = '0.1.0'
  s.summary = 'WebView cujo localhost é o da máquina da sessão.'
  s.description = 'Abre as portas da sessão pelo /api/port-tunnel do servidor, com a origem localhost.'
  s.author = 'Hangar'
  s.homepage = 'https://hangar.dev.br'
  # O app aceita 16.4; o proxy do WKWebView só existe no 17, conferido em tempo de execução.
  s.platforms = { :ios => '16.4' }
  s.swift_version = '5.9'
  s.source = { git: '' }
  s.static_framework = true
  s.dependency 'ExpoModulesCore'
  s.pod_target_xcconfig = { 'DEFINES_MODULE' => 'YES' }
  s.source_files = '**/*.swift'
end

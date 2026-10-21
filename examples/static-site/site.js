const meta = name => document.querySelector(`meta[name="${name}"]`).content;
const client = Analytics.init({
  endpoint: meta('analytics-endpoint'), propertyId: meta('analytics-property'), publicToken: meta('analytics-public-token'),
  profile: 'journey', policyVersion: 1, choice: {enabled: false},
  routes: ['/examples/static-site/', '/examples/static-site/index.html', '/examples/static-site/guide'],
  captureVisibility: true
});
client.ready.catch(() => { document.querySelector('#status').textContent = 'Collection is off. Configure the local service to try this example.'; });
document.querySelector('#minimal').onclick = () => { client.setCollectionChoice({enabled: true, profile: 'minimal'}); document.querySelector('#status').textContent = 'Minimal observations enabled.'; };
document.querySelector('#journey').onclick = () => { client.setCollectionChoice({enabled: true, profile: 'journey'}); document.querySelector('#status').textContent = 'Journey observations enabled for this visit.'; };
document.querySelector('#off').onclick = () => { client.setCollectionChoice({enabled: false}); document.querySelector('#status').textContent = 'Collection is off.'; };
document.querySelector('#guide').onclick = () => history.pushState({}, '', '/examples/static-site/guide');
document.querySelector('form').onsubmit = event => event.preventDefault();
